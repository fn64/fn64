//! In-process authority policy for private release inputs.
//!
//! This module is intentionally split from filesystem mechanics. Every path
//! is opened and measured through `private_fs`; policy never performs a
//! check-then-`fs::read` sequence of its own. The accepted documents and
//! canonical digest wires are the Rust transcription of the repository-owned
//! `tools/private_input_admission.py` v7/v6 admission contract.

use crate::private_fs::{
    check_directory_nofollow, measure_regular_stable, measure_regular_stable_with,
    read_regular_stable, same_lexical_path, validate_absolute_no_parent, PrivateRepository,
    StableFileMeasurement, StableFileRead, StableFileStream,
};
use crate::private_release_series::{
    PrivateArtifactIdentity, PrivateFileIdentity, PrivateReleaseRunContract,
};
#[cfg(test)]
use crate::private_release_series::{
    PrivateChildCommand, PrivateEnvironmentEntry, PRIVATE_RELEASE_RUN_CONTRACT_SCHEMA,
};
use crate::release_program_build_receipt::{
    verify_release_program_build_receipt_document, ReleaseProgramBuildLane,
    ReleaseProgramBuildReceipt, ReleaseProgramFileIdentity,
};
use crate::{ExecutionDestinationSource, ReleaseRomClass};
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
#[cfg(test)]
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;

const MANIFEST_SCHEMA: &str = "fn64.private-input-admission.v7";
const LEGACY_MANIFEST_SCHEMA: &str = "fn64.private-input-admission.v6";
const READINESS_SCHEMA: &str = "fn64.private-input-readiness.v6";
const LEGACY_READINESS_SCHEMA: &str = "fn64.private-input-readiness.v5";
const PROGRAM_BUILD_RECEIPT_SCHEMA: &str = "fn64.release-program-build-receipt.v1";
const MAX_ARTIFACT_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const REPEAT_BAR: u64 = 10;
const F3DZEX2_CHARACTERIZATION_SUITE: &str = "fn64.f3dzex2-point-light.v1";

const EXTENDED_CASES: [&str; 6] = [
    "activation",
    "disabled-negative-control",
    "hook-control",
    "interpolation",
    "vertex-z",
    "widescreen",
];

const F3DZEX2_CHARACTERIZATION_CASES: [&str; 8] = [
    "directional-light-control",
    "lighting-disabled-control",
    "point-light-far-distance",
    "point-light-near-distance",
    "point-light-negative-axis",
    "point-light-positive-axis",
    "point-light-record-boundary",
    "point-light-zero-distance",
];

const RESERVED_RUNNER_ENV: [&str; 9] = [
    "ROM",
    "FN64_RELEASE_GATE_CYCLE",
    "FN64_RELEASE_REPORT",
    "FN64_RELEASE_RUN_EVENT_SHA256",
    "FN64_PRIVATE_RUN_CONTRACT",
    "FN64_PRIVATE_RUN_CONTRACT_SHA256",
    "FN64_PRIVATE_RUN_ORDINAL",
    "FN64_PRIVATE_RUN_ID",
    "FN64_RELEASE_ROM_CLASS",
];

const FORBIDDEN_RUNNER_ENV_PREFIXES: [&str; 20] = [
    "LD_",
    "DYLD_",
    "PYTHON",
    "PERL",
    "RUBY",
    "NODE_",
    "LUA_",
    "TCL_",
    "DOTNET_",
    "MONO_",
    "POWERSHELL_",
    "GTK_",
    "QT_",
    "VK_",
    "LIBGL_",
    "MESA_",
    "__GL_",
    "D3D12SDK",
    "DXVK_",
    "VKD3D_",
];

const FORBIDDEN_RUNNER_ENV: [&str; 19] = [
    "PATH",
    "PATHEXT",
    "COMSPEC",
    "BASH_ENV",
    "ENV",
    "SHELLOPTS",
    "ZDOTDIR",
    "GCONV_PATH",
    "LOCPATH",
    "NLSPATH",
    "CLASSPATH",
    "JAVA_TOOL_OPTIONS",
    "JDK_JAVA_OPTIONS",
    "_JAVA_OPTIONS",
    "SSLKEYLOGFILE",
    "NODE_OPTIONS",
    "GBM_BACKEND",
    "GALLIUM_DRIVER",
    "EGL_PLATFORM",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PrivateInputAdmissionError(String);

impl fmt::Display for PrivateInputAdmissionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for PrivateInputAdmissionError {}

fn error(message: impl Into<String>) -> PrivateInputAdmissionError {
    PrivateInputAdmissionError(message.into())
}

fn map_fs<T, E: fmt::Display>(
    result: Result<T, E>,
    operation: &str,
) -> Result<T, PrivateInputAdmissionError> {
    result.map_err(|source| error(format!("{operation}: {source}")))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Purpose {
    ExtendedGbi,
    F3dzex2Characterization,
    FullRom,
    Combined,
}

impl Purpose {
    const fn wire_name(self) -> &'static str {
        match self {
            Self::ExtendedGbi => "extended_gbi",
            Self::F3dzex2Characterization => "f3dzex2_characterization",
            Self::FullRom => "full_rom",
            Self::Combined => "combined",
        }
    }

    const fn is_private_run(self) -> bool {
        matches!(self, Self::FullRom | Self::Combined)
    }

    const fn requests_extended_gbi(self) -> bool {
        matches!(self, Self::ExtendedGbi | Self::Combined)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum WireFamily {
    F3dex2ExtendedGbiV1,
    F3dex2,
    Fast3dF3dex,
    S2dexS2dex2,
    FullRomMixed,
    F3dzex2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ProgramEvidenceLane {
    NoProgramFixture,
    IdentifiedNativeArchive,
    TypedObservedFunction,
    TypedBlockProgram,
}

impl ProgramEvidenceLane {
    const fn execution_kind(self) -> &'static str {
        match self {
            Self::NoProgramFixture => "no_program",
            Self::IdentifiedNativeArchive => "native_archive",
            Self::TypedObservedFunction => "typed_observed_function_program",
            Self::TypedBlockProgram => "typed_block_program",
        }
    }

    const fn is_authoritative(self) -> bool {
        !matches!(self, Self::NoProgramFixture)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ManifestRomClass {
    RetailCartridge,
    PublicHomebrew,
    NotApplicable,
}

impl ManifestRomClass {
    const fn wire_name(self) -> &'static str {
        match self {
            Self::RetailCartridge => "retail_cartridge",
            Self::PublicHomebrew => "public_homebrew",
            Self::NotApplicable => "not_applicable",
        }
    }

    fn release_class(self) -> Result<ReleaseRomClass, PrivateInputAdmissionError> {
        match self {
            Self::RetailCartridge => Ok(ReleaseRomClass::RetailCartridge),
            Self::PublicHomebrew => Ok(ReleaseRomClass::PublicHomebrew),
            Self::NotApplicable => Err(error(
                "private run contract cannot use rom_class='not_applicable'",
            )),
        }
    }

    const fn expected_rom_provenance(self) -> Option<&'static str> {
        match self {
            Self::RetailCartridge => Some("user_owned_retail_cartridge_dump"),
            Self::PublicHomebrew => Some("publicly_distributed_homebrew_rom"),
            Self::NotApplicable => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Platform {
    #[serde(rename = "macos_arm64")]
    MacosArm64,
    #[serde(rename = "linux_x86_64")]
    LinuxX86_64,
    #[serde(rename = "windows_x86_64")]
    WindowsX86_64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Controller {
    #[serde(rename = "standard_controller")]
    Standard,
    #[serde(rename = "controller_pak")]
    Pak,
    RumblePak,
    TransferPak,
    VoiceRecognitionUnit,
}

impl Controller {
    const fn wire_name(self) -> &'static str {
        match self {
            Self::Standard => "standard_controller",
            Self::Pak => "controller_pak",
            Self::RumblePak => "rumble_pak",
            Self::TransferPak => "transfer_pak",
            Self::VoiceRecognitionUnit => "voice_recognition_unit",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SavePolicy {
    #[serde(rename = "no_cartridge_save")]
    NoCartridgeSave,
    #[serde(rename = "eeprom_4_kbit")]
    Eeprom4Kbit,
    #[serde(rename = "eeprom_16_kbit")]
    Eeprom16Kbit,
    #[serde(rename = "sram_32_kib")]
    Sram32Kib,
    #[serde(rename = "flash_ram_128_kib")]
    FlashRam128Kib,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Renderer {
    ReferenceLleAccuracy,
    Rt64LleAccuracy,
    Rt64PostViCapture,
    Rt64ReplacementPacks,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ArtifactRole {
    MicrocodeData,
    MicrocodeDataRawWindow,
    MicrocodeText,
    MicrocodeTextRawWindow,
    Recompiled,
    Rom,
}

impl ArtifactRole {
    const fn wire_name(self) -> &'static str {
        match self {
            Self::MicrocodeData => "microcode_data",
            Self::MicrocodeDataRawWindow => "microcode_data_raw_window",
            Self::MicrocodeText => "microcode_text",
            Self::MicrocodeTextRawWindow => "microcode_text_raw_window",
            Self::Recompiled => "recompiled",
            Self::Rom => "rom",
        }
    }

    fn valid_provenance(self, provenance: &str) -> bool {
        match self {
            Self::MicrocodeData
            | Self::MicrocodeDataRawWindow
            | Self::MicrocodeText
            | Self::MicrocodeTextRawWindow => provenance == "user_owned_rom_derived",
            Self::Recompiled => provenance == "user_generated_from_owned_rom",
            Self::Rom => matches!(
                provenance,
                "user_owned_retail_cartridge_dump" | "publicly_distributed_homebrew_rom"
            ),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct IntentV7 {
    wire_family: WireFamily,
    report_scenario: String,
    recognition: String,
    extended_gbi_cases: Vec<String>,
    program_evidence_lane: ProgramEvidenceLane,
    rom_class: ManifestRomClass,
    #[serde(deserialize_with = "deserialize_present_option")]
    characterization_suite: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct IntentV6 {
    wire_family: WireFamily,
    report_scenario: String,
    recognition: String,
    extended_gbi_cases: Vec<String>,
    program_evidence_lane: ProgramEvidenceLane,
    rom_class: ManifestRomClass,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleaseMatrixPolicy {
    platform: Platform,
    controllers: Vec<Controller>,
    save: SavePolicy,
    renderers: Vec<Renderer>,
    repeat_bar: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArtifactDescriptor {
    path: String,
    length: u64,
    sha256: String,
    provenance: String,
    git_identity: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecutableDescriptor {
    path: String,
    length: u64,
    sha256: String,
    git_identity: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArtifactsV7 {
    #[serde(deserialize_with = "deserialize_present_option")]
    microcode_text: Option<ArtifactDescriptor>,
    #[serde(deserialize_with = "deserialize_present_option")]
    microcode_data: Option<ArtifactDescriptor>,
    #[serde(deserialize_with = "deserialize_present_option")]
    microcode_text_raw_window: Option<ArtifactDescriptor>,
    #[serde(deserialize_with = "deserialize_present_option")]
    microcode_data_raw_window: Option<ArtifactDescriptor>,
    #[serde(deserialize_with = "deserialize_present_option")]
    rom: Option<ArtifactDescriptor>,
    #[serde(deserialize_with = "deserialize_present_option")]
    recompiled: Option<ArtifactDescriptor>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArtifactsV6 {
    #[serde(deserialize_with = "deserialize_present_option")]
    microcode_text: Option<ArtifactDescriptor>,
    #[serde(deserialize_with = "deserialize_present_option")]
    microcode_data: Option<ArtifactDescriptor>,
    #[serde(deserialize_with = "deserialize_present_option")]
    rom: Option<ArtifactDescriptor>,
    #[serde(deserialize_with = "deserialize_present_option")]
    recompiled: Option<ArtifactDescriptor>,
}

fn deserialize_present_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
struct StrictEnvironment(BTreeMap<String, String>);

impl Serialize for StrictEnvironment {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for StrictEnvironment {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct EnvironmentVisitor;

        impl<'de> Visitor<'de> for EnvironmentVisitor {
            type Value = StrictEnvironment;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an environment object with unique names")
            }

            fn visit_map<A>(self, mut access: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut values = BTreeMap::new();
                while let Some((name, value)) = access.next_entry::<String, String>()? {
                    if values.insert(name.clone(), value).is_some() {
                        return Err(de::Error::custom(format!(
                            "JSON object contains duplicate field {name:?}"
                        )));
                    }
                }
                Ok(StrictEnvironment(values))
            }
        }

        deserializer.deserialize_map(EnvironmentVisitor)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunnerPolicy {
    executable: ExecutableDescriptor,
    working_directory: String,
    argv: Vec<String>,
    env: StrictEnvironment,
    release_gate_cycle: u64,
    execution_source: ExecutionDestinationSource,
    #[serde(deserialize_with = "deserialize_present_option")]
    program_build_receipt: Option<ExecutableDescriptor>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestV7 {
    purpose: Purpose,
    intent: IntentV7,
    release_matrix: ReleaseMatrixPolicy,
    artifacts: ArtifactsV7,
    runner: RunnerPolicy,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestV6 {
    purpose: Purpose,
    intent: IntentV6,
    release_matrix: ReleaseMatrixPolicy,
    artifacts: ArtifactsV6,
    runner: RunnerPolicy,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "schema")]
enum StoredManifest {
    #[serde(rename = "fn64.private-input-admission.v7")]
    V7(Box<ManifestV7>),
    #[serde(rename = "fn64.private-input-admission.v6")]
    V6(Box<ManifestV6>),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadinessV6 {
    status: String,
    purpose: Purpose,
    wire_family: WireFamily,
    report_scenario: String,
    rom_class: ManifestRomClass,
    program_evidence_lane: ProgramEvidenceLane,
    artifact_roles_admitted: Vec<ArtifactRole>,
    extended_gbi_fixture: String,
    full_rom_inputs: String,
    program_build_receipt: String,
    release_matrix_policy: String,
    repeat_bar: u64,
    required_extended_cases: Vec<String>,
    platform: Platform,
    controllers: Vec<Controller>,
    save: SavePolicy,
    renderers: Vec<Renderer>,
    characterization_fixture: String,
    characterization_suite: String,
    characterization_vector_source: String,
    required_characterization_cases: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadinessV5 {
    status: String,
    purpose: Purpose,
    wire_family: WireFamily,
    report_scenario: String,
    rom_class: ManifestRomClass,
    program_evidence_lane: ProgramEvidenceLane,
    artifact_roles_admitted: Vec<ArtifactRole>,
    extended_gbi_fixture: String,
    full_rom_inputs: String,
    program_build_receipt: String,
    release_matrix_policy: String,
    repeat_bar: u64,
    required_extended_cases: Vec<String>,
    platform: Platform,
    controllers: Vec<Controller>,
    save: SavePolicy,
    renderers: Vec<Renderer>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "schema")]
enum StoredReadiness {
    #[serde(rename = "fn64.private-input-readiness.v6")]
    V6(ReadinessV6),
    #[serde(rename = "fn64.private-input-readiness.v5")]
    V5(ReadinessV5),
}

struct AdmittedArtifact {
    descriptor: ArtifactDescriptor,
    measurement: StableFileMeasurement,
    captured_contents: Option<Vec<u8>>,
}

struct ValidatedManifest {
    schema: &'static str,
    purpose: Purpose,
    wire_family: WireFamily,
    report_scenario: String,
    program_lane: ProgramEvidenceLane,
    rom_class: ManifestRomClass,
    release: ReleaseMatrixPolicy,
    artifacts: BTreeMap<ArtifactRole, AdmittedArtifact>,
    runner: RunnerPolicy,
    executable: StableFileMeasurement,
    program_receipt: Option<StableFileMeasurement>,
}

/// Canonical payloads for current-schema admission. Publication remains the
/// caller's create-new operation; these bytes are already policy-validated.
#[derive(Clone, Debug)]
#[cfg(test)]
pub(crate) struct CurrentPrivateAdmissionPayloads {
    pub(crate) readiness_bytes: Vec<u8>,
    pub(crate) contract_bytes: Option<Vec<u8>>,
    pub(crate) contract: Option<PrivateReleaseRunContract>,
}

/// Opaque, stable-handle capture of the two raw windows admitted for the
/// repository-owned F3DZEX2 characterization suite.
pub struct VerifiedPrivateF3dzex2CharacterizationInput {
    text_raw_window: Box<[u8; 0x18d0]>,
    data_raw_window: Box<[u8; 0x0fc0]>,
}

impl VerifiedPrivateF3dzex2CharacterizationInput {
    pub fn raw_text_window(&self) -> &[u8] {
        &*self.text_raw_window
    }

    pub fn raw_data_window(&self) -> &[u8] {
        &*self.data_raw_window
    }
}

/// Content-free public failure for private characterization admission. The
/// detailed policy error can contain private paths and therefore remains
/// inside this crate.
pub struct PrivateF3dzex2CharacterizationError;

impl fmt::Debug for PrivateF3dzex2CharacterizationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PrivateF3dzex2CharacterizationError")
    }
}

impl fmt::Display for PrivateF3dzex2CharacterizationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("private F3DZEX2 characterization admission failed")
    }
}

impl std::error::Error for PrivateF3dzex2CharacterizationError {}

/// Revalidate a current F3DZEX2 characterization manifest and its canonical
/// content-free readiness report, returning only the raw bytes captured from
/// the stable descriptors used by admission.
pub fn load_private_f3dzex2_characterization_input(
    manifest_path: impl AsRef<Path>,
    readiness_path: impl AsRef<Path>,
) -> Result<VerifiedPrivateF3dzex2CharacterizationInput, PrivateF3dzex2CharacterizationError> {
    load_private_f3dzex2_characterization_input_inner(
        manifest_path.as_ref(),
        readiness_path.as_ref(),
    )
    .map_err(|_| PrivateF3dzex2CharacterizationError)
}

fn validate_current_v7_manifest(
    repository: &PrivateRepository,
    manifest_contents: &[u8],
) -> Result<(ValidatedManifest, Vec<u8>), PrivateInputAdmissionError> {
    let stored = parse_manifest(manifest_contents, "manifest")?;
    let StoredManifest::V7(manifest) = stored else {
        return Err(error(format!(
            "new admission requires schema {MANIFEST_SCHEMA:?}; retained {LEGACY_MANIFEST_SCHEMA:?} is read-only"
        )));
    };
    let validated = validate_manifest_v7(repository, *manifest)?;
    let readiness = derive_readiness(&validated)?;
    validate_readiness(&readiness)?;
    let readiness_bytes = serialize_json_document(&readiness, "readiness report")?;
    Ok((validated, readiness_bytes))
}

fn load_private_f3dzex2_characterization_input_inner(
    manifest_path: &Path,
    readiness_path: &Path,
) -> Result<VerifiedPrivateF3dzex2CharacterizationInput, PrivateInputAdmissionError> {
    let repository = map_fs(PrivateRepository::discover(), "discover fn64 repository")?;
    let manifest_read = read_private_file(&repository, manifest_path, "manifest")?;
    let (mut validated, expected_readiness) =
        validate_current_v7_manifest(&repository, &manifest_read.contents)?;
    if validated.purpose != Purpose::F3dzex2Characterization {
        return Err(error(
            "private admission purpose is not f3dzex2_characterization",
        ));
    }

    let supplied_readiness = read_private_file(&repository, readiness_path, "readiness report")?;
    if supplied_readiness.contents != expected_readiness {
        return Err(error(
            "supplied readiness does not match current F3DZEX2 admission",
        ));
    }

    let mut take_window = |role: ArtifactRole| {
        validated
            .artifacts
            .remove(&role)
            .and_then(|artifact| artifact.captured_contents)
            .ok_or_else(|| {
                error(format!(
                    "admitted {} bytes were not captured from the stable descriptor",
                    role.wire_name()
                ))
            })
    };
    let text_raw_window = take_window(ArtifactRole::MicrocodeTextRawWindow)?
        .into_boxed_slice()
        .try_into()
        .map_err(|_| error("admitted F3DZEX2 text window geometry changed after validation"))?;
    let data_raw_window = take_window(ArtifactRole::MicrocodeDataRawWindow)?
        .into_boxed_slice()
        .try_into()
        .map_err(|_| error("admitted F3DZEX2 data window geometry changed after validation"))?;
    Ok(VerifiedPrivateF3dzex2CharacterizationInput {
        text_raw_window,
        data_raw_window,
    })
}

/// Opaque production authority reconstructed by replaying a retained v3
/// contract against its v7/v6 manifest, v6/v5 readiness, receipt, and files.
#[derive(Debug)]
pub(crate) struct AdmittedProductionContract {
    contract: PrivateReleaseRunContract,
}

impl AdmittedProductionContract {
    pub(crate) fn into_contract(self) -> PrivateReleaseRunContract {
        self.contract
    }
}

/// Admit a current v7 manifest and derive content-free readiness plus, for a
/// production purpose, the v3 private contract. Retained v6 is deliberately
/// rejected here and accepted only by retained-contract verification.
#[cfg(test)]
pub(crate) fn admit_current_v7_manifest(
    manifest_path: &Path,
    readiness_path: &Path,
) -> Result<CurrentPrivateAdmissionPayloads, PrivateInputAdmissionError> {
    let repository = map_fs(PrivateRepository::discover(), "discover fn64 repository")?;
    validate_private_output_path(&repository, readiness_path, "readiness output")?;
    let manifest_read = read_private_file(&repository, manifest_path, "manifest")?;
    let (validated, readiness_bytes) =
        validate_current_v7_manifest(&repository, &manifest_read.contents)?;

    let (contract, contract_bytes) = if validated.purpose.is_private_run() {
        let mut contract = build_private_run_contract(
            &validated,
            &manifest_read.measurement,
            readiness_path,
            &readiness_bytes,
        )?;
        contract.contract_sha256 = contract.recompute_contract_sha256().map_err(|source| {
            error(format!(
                "compute private run contract canonical digest: {source}"
            ))
        })?;
        contract
            .verify_integrity()
            .map_err(|source| error(format!("verify derived private run contract: {source}")))?;
        let bytes = serialize_json_document(&contract, "private run contract")?;
        (Some(contract), Some(bytes))
    } else {
        (None, None)
    };

    Ok(CurrentPrivateAdmissionPayloads {
        readiness_bytes,
        contract_bytes,
        contract,
    })
}

/// Replay retained production admission over one stable capture of the
/// contract. Replacement of the source path after this capture is irrelevant:
/// policy parses the retained handle's bytes and never reopens that path.
pub(crate) fn verify_retained_private_run_contract(
    contract_path: &Path,
) -> Result<AdmittedProductionContract, PrivateInputAdmissionError> {
    let repository = map_fs(PrivateRepository::discover(), "discover fn64 repository")?;
    let contract_read = read_private_file(&repository, contract_path, "private run contract")?;
    verify_retained_contract_read(&repository, contract_read)
}

fn verify_retained_contract_read(
    repository: &PrivateRepository,
    contract_read: StableFileRead,
) -> Result<AdmittedProductionContract, PrivateInputAdmissionError> {
    let contract: PrivateReleaseRunContract = serde_json::from_slice(&contract_read.contents)
        .map_err(|source| error(format!("parse private run contract: {source}")))?;
    contract.verify_integrity().map_err(|source| {
        error(format!(
            "verify private run contract canonical wire: {source}"
        ))
    })?;
    if !matches!(contract.purpose.as_str(), "full_rom" | "combined") {
        return Err(error(
            "production private run contract purpose must be full_rom or combined",
        ));
    }

    let manifest_read = read_bound_descriptor(
        repository,
        &contract.admission_manifest,
        "contract.admission_manifest",
    )?;
    let stored_manifest = parse_manifest(&manifest_read.contents, "admission manifest")?;
    let validated = match stored_manifest {
        StoredManifest::V7(manifest) => validate_manifest_v7(repository, *manifest)?,
        StoredManifest::V6(manifest) => validate_manifest_v6(repository, *manifest)?,
    };
    let derived_readiness = derive_readiness(&validated)?;
    validate_readiness(&derived_readiness)?;

    let readiness_read = read_bound_descriptor(
        repository,
        &contract.readiness_report,
        "contract.readiness_report",
    )?;
    let retained_readiness: StoredReadiness = serde_json::from_slice(&readiness_read.contents)
        .map_err(|source| error(format!("parse retained readiness report: {source}")))?;
    validate_readiness(&retained_readiness)?;
    if retained_readiness != derived_readiness {
        return Err(error(
            "contract readiness report does not match its validated manifest",
        ));
    }

    cross_bind_contract(repository, &contract, &validated)?;
    Ok(AdmittedProductionContract { contract })
}

fn parse_manifest(bytes: &[u8], field: &str) -> Result<StoredManifest, PrivateInputAdmissionError> {
    serde_json::from_slice(bytes).map_err(|source| error(format!("parse {field}: {source}")))
}

fn validate_manifest_v7(
    repository: &PrivateRepository,
    manifest: ManifestV7,
) -> Result<ValidatedManifest, PrivateInputAdmissionError> {
    validate_manifest_common(
        repository,
        MANIFEST_SCHEMA,
        manifest.purpose,
        manifest.intent.wire_family,
        manifest.intent.report_scenario,
        manifest.intent.recognition,
        manifest.intent.extended_gbi_cases,
        manifest.intent.program_evidence_lane,
        manifest.intent.rom_class,
        manifest.intent.characterization_suite,
        manifest.release_matrix,
        vec![
            (
                ArtifactRole::MicrocodeText,
                manifest.artifacts.microcode_text,
            ),
            (
                ArtifactRole::MicrocodeData,
                manifest.artifacts.microcode_data,
            ),
            (
                ArtifactRole::MicrocodeTextRawWindow,
                manifest.artifacts.microcode_text_raw_window,
            ),
            (
                ArtifactRole::MicrocodeDataRawWindow,
                manifest.artifacts.microcode_data_raw_window,
            ),
            (ArtifactRole::Rom, manifest.artifacts.rom),
            (ArtifactRole::Recompiled, manifest.artifacts.recompiled),
        ],
        manifest.runner,
    )
}

fn validate_manifest_v6(
    repository: &PrivateRepository,
    manifest: ManifestV6,
) -> Result<ValidatedManifest, PrivateInputAdmissionError> {
    if matches!(manifest.purpose, Purpose::F3dzex2Characterization)
        || matches!(manifest.intent.wire_family, WireFamily::F3dzex2)
    {
        return Err(error(
            "retained v6 manifest cannot select F3DZEX2 characterization",
        ));
    }
    validate_manifest_common(
        repository,
        LEGACY_MANIFEST_SCHEMA,
        manifest.purpose,
        manifest.intent.wire_family,
        manifest.intent.report_scenario,
        manifest.intent.recognition,
        manifest.intent.extended_gbi_cases,
        manifest.intent.program_evidence_lane,
        manifest.intent.rom_class,
        None,
        manifest.release_matrix,
        vec![
            (
                ArtifactRole::MicrocodeText,
                manifest.artifacts.microcode_text,
            ),
            (
                ArtifactRole::MicrocodeData,
                manifest.artifacts.microcode_data,
            ),
            (ArtifactRole::Rom, manifest.artifacts.rom),
            (ArtifactRole::Recompiled, manifest.artifacts.recompiled),
        ],
        manifest.runner,
    )
}

#[allow(clippy::too_many_arguments)]
fn validate_manifest_common(
    repository: &PrivateRepository,
    schema: &'static str,
    purpose: Purpose,
    wire_family: WireFamily,
    report_scenario: String,
    recognition: String,
    extended_gbi_cases: Vec<String>,
    program_lane: ProgramEvidenceLane,
    rom_class: ManifestRomClass,
    characterization_suite: Option<String>,
    release: ReleaseMatrixPolicy,
    descriptors: Vec<(ArtifactRole, Option<ArtifactDescriptor>)>,
    runner: RunnerPolicy,
) -> Result<ValidatedManifest, PrivateInputAdmissionError> {
    if schema == LEGACY_MANIFEST_SCHEMA && matches!(purpose, Purpose::F3dzex2Characterization) {
        return Err(error(
            "retained v6 manifest does not admit f3dzex2_characterization",
        ));
    }
    if schema == LEGACY_MANIFEST_SCHEMA && matches!(wire_family, WireFamily::F3dzex2) {
        return Err(error(
            "retained v6 manifest does not admit wire family f3dzex2",
        ));
    }
    validate_scenario(&report_scenario, "intent.report_scenario")?;
    if recognition != "runtime_must_confirm_backend_known_pair" {
        return Err(error(
            "intent.recognition must preserve the exact backend text/data-pair gate",
        ));
    }
    validate_unique_strings(&extended_gbi_cases, "intent.extended_gbi_cases")?;
    let extended_set = extended_gbi_cases
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let exact_extended = EXTENDED_CASES.into_iter().collect::<BTreeSet<_>>();
    if purpose.requests_extended_gbi() {
        if wire_family != WireFamily::F3dex2ExtendedGbiV1 || extended_set != exact_extended {
            return Err(error(
                "Extended GBI requires f3dex2_extended_gbi_v1 and the exact six-case denominator",
            ));
        }
    } else if !extended_set.is_empty() {
        return Err(error(format!(
            "{} admission must not claim Extended GBI cases",
            purpose.wire_name()
        )));
    }
    if purpose == Purpose::F3dzex2Characterization {
        if schema != MANIFEST_SCHEMA
            || wire_family != WireFamily::F3dzex2
            || characterization_suite.as_deref() != Some(F3DZEX2_CHARACTERIZATION_SUITE)
        {
            return Err(error(
                "F3DZEX2 characterization requires current schema, f3dzex2, and the repository-owned point-light suite",
            ));
        }
    } else if schema == MANIFEST_SCHEMA && characterization_suite.is_some() {
        return Err(error(format!(
            "{} admission must set intent.characterization_suite to null",
            purpose.wire_name()
        )));
    }

    validate_release_policy(purpose, &release)?;
    let mut artifacts = BTreeMap::new();
    for (role, descriptor) in descriptors {
        if let Some(descriptor) = descriptor {
            let artifact = validate_artifact(repository, role, descriptor)?;
            artifacts.insert(role, artifact);
        }
    }
    validate_artifact_denominator(purpose, program_lane, rom_class, &artifacts)?;
    let (executable, program_receipt) = validate_runner(
        repository,
        &runner,
        program_lane,
        artifacts.get(&ArtifactRole::Recompiled),
    )?;

    Ok(ValidatedManifest {
        schema,
        purpose,
        wire_family,
        report_scenario,
        program_lane,
        rom_class,
        release,
        artifacts,
        runner,
        executable,
        program_receipt,
    })
}

fn validate_release_policy(
    purpose: Purpose,
    release: &ReleaseMatrixPolicy,
) -> Result<(), PrivateInputAdmissionError> {
    validate_unique(&release.controllers, "release_matrix.controllers")?;
    if release.controllers.is_empty() {
        return Err(error("release_matrix.controllers must not be empty"));
    }
    validate_unique(&release.renderers, "release_matrix.renderers")?;
    let renderers = release.renderers.iter().copied().collect::<BTreeSet<_>>();
    if renderers.is_empty() {
        return Err(error("release_matrix.renderers must not be empty"));
    }
    if renderers.contains(&Renderer::ReferenceLleAccuracy) {
        if renderers != BTreeSet::from([Renderer::ReferenceLleAccuracy]) {
            return Err(error("reference LLE must stand alone"));
        }
    } else if !renderers.contains(&Renderer::Rt64LleAccuracy) {
        return Err(error("RT64 renderer coverage requires rt64_lle_accuracy"));
    }
    if (purpose.requests_extended_gbi() || purpose == Purpose::F3dzex2Characterization)
        && !(renderers.contains(&Renderer::Rt64LleAccuracy)
            && renderers.contains(&Renderer::Rt64PostViCapture))
    {
        return Err(error(format!(
            "{} requires RT64 LLE and post-VI capture coverage",
            purpose.wire_name()
        )));
    }
    if release.repeat_bar != REPEAT_BAR {
        return Err(error("release_matrix.repeat_bar must be exactly 10"));
    }
    Ok(())
}

fn validate_artifact(
    repository: &PrivateRepository,
    role: ArtifactRole,
    descriptor: ArtifactDescriptor,
) -> Result<AdmittedArtifact, PrivateInputAdmissionError> {
    if descriptor.git_identity != "excluded" {
        return Err(error(format!(
            "artifacts.{}.git_identity must be 'excluded'",
            role.wire_name()
        )));
    }
    if !role.valid_provenance(&descriptor.provenance) {
        return Err(error(format!(
            "artifacts.{}.provenance is invalid",
            role.wire_name()
        )));
    }
    validate_positive_length(
        descriptor.length,
        &format!("artifacts.{}.length", role.wire_name()),
    )?;
    match role {
        ArtifactRole::MicrocodeText if descriptor.length != 4096 => {
            return Err(error(
                "artifacts.microcode_text.length must be the exact 4 KiB IMEM image",
            ));
        }
        ArtifactRole::MicrocodeTextRawWindow if descriptor.length != 0x18d0 => {
            return Err(error(
                "artifacts.microcode_text_raw_window.length must be exactly 0x18d0",
            ));
        }
        ArtifactRole::MicrocodeDataRawWindow if descriptor.length != 0x0fc0 => {
            return Err(error(
                "artifacts.microcode_data_raw_window.length must be exactly 0x0fc0",
            ));
        }
        _ => {}
    }
    require_sha256(
        &descriptor.sha256,
        &format!("artifacts.{}.sha256", role.wire_name()),
    )?;
    let field = format!("artifacts.{}", role.wire_name());
    let (measurement, captured_contents) = if matches!(
        role,
        ArtifactRole::MicrocodeTextRawWindow | ArtifactRole::MicrocodeDataRawWindow
    ) {
        let read = read_private_file(repository, Path::new(&descriptor.path), &field)?;
        (read.measurement, Some(read.contents))
    } else {
        (
            measure_private_file(repository, Path::new(&descriptor.path), &field)?,
            None,
        )
    };
    require_measurement(&measurement, descriptor.length, &descriptor.sha256, &field)?;
    Ok(AdmittedArtifact {
        descriptor,
        measurement,
        captured_contents,
    })
}

fn validate_artifact_denominator(
    purpose: Purpose,
    program_lane: ProgramEvidenceLane,
    rom_class: ManifestRomClass,
    artifacts: &BTreeMap<ArtifactRole, AdmittedArtifact>,
) -> Result<(), PrivateInputAdmissionError> {
    let roles = artifacts.keys().copied().collect::<BTreeSet<_>>();
    if purpose == Purpose::F3dzex2Characterization {
        let exact = BTreeSet::from([
            ArtifactRole::MicrocodeDataRawWindow,
            ArtifactRole::MicrocodeTextRawWindow,
        ]);
        if roles != exact {
            return Err(error(
                "F3DZEX2 characterization admits exactly the two native raw recognition windows",
            ));
        }
    } else {
        if !roles.contains(&ArtifactRole::MicrocodeText)
            || !roles.contains(&ArtifactRole::MicrocodeData)
        {
            return Err(error(
                "logical admission requires microcode_text and microcode_data",
            ));
        }
        if roles.contains(&ArtifactRole::MicrocodeTextRawWindow)
            || roles.contains(&ArtifactRole::MicrocodeDataRawWindow)
        {
            return Err(error(
                "non-characterization admission cannot contain raw recognition windows",
            ));
        }
        if artifacts[&ArtifactRole::MicrocodeData].descriptor.length > u64::from(u32::MAX) {
            return Err(error(
                "artifacts.microcode_data length exceeds the task-header u32 size field",
            ));
        }
    }

    if purpose.is_private_run() {
        if !roles.contains(&ArtifactRole::Rom) || !roles.contains(&ArtifactRole::Recompiled) {
            return Err(error(format!(
                "{} admission requires ROM and recompiled artifacts",
                purpose.wire_name()
            )));
        }
        if rom_class == ManifestRomClass::NotApplicable {
            return Err(error(format!(
                "{} admission requires a retail_cartridge or public_homebrew ROM class",
                purpose.wire_name()
            )));
        }
        let rom = artifacts
            .get(&ArtifactRole::Rom)
            .expect("private role presence checked above");
        if rom_class.expected_rom_provenance() != Some(rom.descriptor.provenance.as_str()) {
            return Err(error(format!(
                "artifacts.rom.provenance does not match ROM class {:?}",
                rom_class.wire_name()
            )));
        }
        if !program_lane.is_authoritative() {
            return Err(error(format!(
                "{} admission requires an authoritative executable lane",
                purpose.wire_name()
            )));
        }
    } else if rom_class != ManifestRomClass::NotApplicable
        || program_lane != ProgramEvidenceLane::NoProgramFixture
    {
        return Err(error(format!(
            "{} fixture admission requires rom_class='not_applicable' and no_program_fixture",
            purpose.wire_name()
        )));
    }
    Ok(())
}

fn validate_runner(
    repository: &PrivateRepository,
    runner: &RunnerPolicy,
    lane: ProgramEvidenceLane,
    recompiled: Option<&AdmittedArtifact>,
) -> Result<(StableFileMeasurement, Option<StableFileMeasurement>), PrivateInputAdmissionError> {
    validate_executable_descriptor(&runner.executable, "runner.executable")?;
    let executable = measure_private_executable(
        repository,
        Path::new(&runner.executable.path),
        "runner.executable",
    )?;
    require_measurement(
        &executable,
        runner.executable.length,
        &runner.executable.sha256,
        "runner.executable",
    )?;

    let working_directory = Path::new(&runner.working_directory);
    map_fs(
        validate_absolute_no_parent(working_directory, "runner.working_directory"),
        "validate runner.working_directory",
    )?;
    map_fs(
        check_directory_nofollow(working_directory, "runner.working_directory"),
        "inspect runner.working_directory",
    )?;
    map_fs(
        repository.require_outside_or_gitignored(working_directory, "runner.working_directory"),
        "exclude runner.working_directory from git",
    )?;

    for (index, argument) in runner.argv.iter().enumerate() {
        if argument.is_empty() || argument.contains('\0') {
            return Err(error(format!(
                "runner.argv[{index}] must be a nonempty string without NUL"
            )));
        }
    }
    for (name, value) in &runner.env.0 {
        validate_environment_name(name)?;
        if value.contains('\0') {
            return Err(error(format!(
                "runner.env[{name:?}] must be a string without NUL"
            )));
        }
    }
    validate_execution_source(&runner.execution_source, lane, "runner.execution_source")?;

    let receipt = match (&runner.program_build_receipt, lane.is_authoritative()) {
        (None, false) => None,
        (Some(_), false) => {
            return Err(error(
                "no_program_fixture runner cannot bind a program-build receipt",
            ));
        }
        (None, true) => {
            return Err(error(
                "authoritative program lane requires runner.program_build_receipt",
            ));
        }
        (Some(descriptor), true) => {
            validate_executable_descriptor(descriptor, "runner.program_build_receipt")?;
            let receipt_read = read_private_file(
                repository,
                Path::new(&descriptor.path),
                "runner.program_build_receipt",
            )?;
            require_measurement(
                &receipt_read.measurement,
                descriptor.length,
                &descriptor.sha256,
                "runner.program_build_receipt",
            )?;
            let receipt: ReleaseProgramBuildReceipt =
                serde_json::from_slice(&receipt_read.contents)
                    .map_err(|source| error(format!("parse program-build receipt: {source}")))?;
            require_receipt_private_paths(repository, &receipt)?;
            let verified = verify_release_program_build_receipt_document(&receipt_read.contents)
                .map_err(|source| {
                    error(format!("verify {PROGRAM_BUILD_RECEIPT_SCHEMA}: {source}"))
                })?;
            verify_receipt_binding(
                &verified.receipt,
                &verified.recomputed_execution_source,
                lane,
                &runner.executable,
                &runner.execution_source,
                recompiled.ok_or_else(|| {
                    error("authoritative program lane requires artifacts.recompiled")
                })?,
            )?;
            Some(receipt_read.measurement)
        }
    };

    Ok((executable, receipt))
}

fn require_receipt_private_paths(
    repository: &PrivateRepository,
    receipt: &ReleaseProgramBuildReceipt,
) -> Result<(), PrivateInputAdmissionError> {
    require_receipt_private_path(
        repository,
        &receipt.child_executable,
        "program receipt.child_executable",
    )?;
    match &receipt.lane {
        ReleaseProgramBuildLane::NativeArchives { archives } => {
            for (index, archive) in archives.iter().enumerate() {
                require_receipt_private_path(
                    repository,
                    &archive.file,
                    &format!("program receipt.lane.archives[{index}].file"),
                )?;
            }
        }
        ReleaseProgramBuildLane::TypedObservedFunction { identity_wire } => {
            require_receipt_private_path(
                repository,
                identity_wire,
                "program receipt.lane.identity_wire",
            )?;
        }
        ReleaseProgramBuildLane::TypedBlock { pack, .. } => {
            require_receipt_private_path(repository, pack, "program receipt.lane.pack")?;
        }
    }
    Ok(())
}

fn require_receipt_private_path(
    repository: &PrivateRepository,
    identity: &ReleaseProgramFileIdentity,
    field: &str,
) -> Result<(), PrivateInputAdmissionError> {
    let path = Path::new(&identity.path);
    path_to_utf8(path, field)?;
    map_fs(
        validate_absolute_no_parent(path, field),
        &format!("validate {field}"),
    )?;
    map_fs(
        repository.require_outside_or_gitignored(path, field),
        &format!("exclude {field} from git"),
    )
}

fn verify_receipt_binding(
    receipt: &ReleaseProgramBuildReceipt,
    recomputed_source: &ExecutionDestinationSource,
    lane: ProgramEvidenceLane,
    expected_child: &ExecutableDescriptor,
    expected_source: &ExecutionDestinationSource,
    recompiled: &AdmittedArtifact,
) -> Result<(), PrivateInputAdmissionError> {
    if receipt.schema != PROGRAM_BUILD_RECEIPT_SCHEMA {
        return Err(error("program-build receipt schema is invalid"));
    }
    if !same_identity_with_executable(&receipt.child_executable, expected_child)? {
        return Err(error(
            "program-build receipt child does not match runner.executable",
        ));
    }
    let matching = match (&receipt.lane, lane) {
        (
            ReleaseProgramBuildLane::NativeArchives { archives },
            ProgramEvidenceLane::IdentifiedNativeArchive,
        ) => archives
            .iter()
            .filter(|archive| same_identity_with_artifact(&archive.file, recompiled))
            .count(),
        (
            ReleaseProgramBuildLane::TypedObservedFunction { identity_wire },
            ProgramEvidenceLane::TypedObservedFunction,
        ) => usize::from(same_identity_with_artifact(identity_wire, recompiled)),
        (
            ReleaseProgramBuildLane::TypedBlock { pack, .. },
            ProgramEvidenceLane::TypedBlockProgram,
        ) => usize::from(same_identity_with_artifact(pack, recompiled)),
        _ => {
            return Err(error(
                "program-build receipt lane does not match manifest lane",
            ));
        }
    };
    if matching != 1 {
        return Err(error(
            "program-build receipt must bind exactly one lane input equal to artifacts.recompiled",
        ));
    }
    if &receipt.expected_execution_source != recomputed_source
        || recomputed_source != expected_source
    {
        return Err(error(
            "program-build receipt execution source does not match recomputed and runner identities",
        ));
    }
    Ok(())
}

fn same_identity_with_executable(
    identity: &ReleaseProgramFileIdentity,
    descriptor: &ExecutableDescriptor,
) -> Result<bool, PrivateInputAdmissionError> {
    Ok(
        same_lexical_path(Path::new(&identity.path), Path::new(&descriptor.path))
            && identity.bytes == descriptor.length
            && identity.sha256 == descriptor.sha256,
    )
}

fn same_identity_with_artifact(
    identity: &ReleaseProgramFileIdentity,
    artifact: &AdmittedArtifact,
) -> bool {
    identity.path == artifact.descriptor.path
        && identity.bytes == artifact.descriptor.length
        && identity.sha256 == artifact.descriptor.sha256
}

fn derive_readiness(
    manifest: &ValidatedManifest,
) -> Result<StoredReadiness, PrivateInputAdmissionError> {
    let mut roles = manifest.artifacts.keys().copied().collect::<Vec<_>>();
    roles.sort_unstable_by_key(|role| role.wire_name());
    let mut controllers = manifest.release.controllers.clone();
    controllers.sort_unstable_by_key(|controller| controller.wire_name());
    let mut renderers = manifest.release.renderers.clone();
    renderers.sort_unstable();
    let required_extended_cases = if manifest.purpose.requests_extended_gbi() {
        EXTENDED_CASES
            .iter()
            .map(|value| (*value).to_owned())
            .collect()
    } else {
        Vec::new()
    };
    let common = (
        "ready".to_owned(),
        manifest.purpose,
        manifest.wire_family,
        manifest.report_scenario.clone(),
        roles,
        if manifest.purpose.requests_extended_gbi() {
            "ready_for_runtime_recognition"
        } else {
            "not_requested"
        }
        .to_owned(),
        if manifest.artifacts.contains_key(&ArtifactRole::Rom)
            && manifest.artifacts.contains_key(&ArtifactRole::Recompiled)
        {
            "ready"
        } else {
            "not_supplied"
        }
        .to_owned(),
        "ready_for_ten_run_evidence".to_owned(),
        REPEAT_BAR,
        required_extended_cases,
        manifest.release.platform,
        controllers,
        manifest.release.save,
        renderers,
        manifest.program_lane,
        if manifest.program_lane.is_authoritative() {
            "verified"
        } else {
            "not_applicable"
        }
        .to_owned(),
        manifest.rom_class,
    );

    if manifest.schema == MANIFEST_SCHEMA {
        let characterization = manifest.purpose == Purpose::F3dzex2Characterization;
        Ok(StoredReadiness::V6(ReadinessV6 {
            status: common.0,
            purpose: common.1,
            wire_family: common.2,
            report_scenario: common.3,
            artifact_roles_admitted: common.4,
            extended_gbi_fixture: common.5,
            full_rom_inputs: common.6,
            release_matrix_policy: common.7,
            repeat_bar: common.8,
            required_extended_cases: common.9,
            platform: common.10,
            controllers: common.11,
            save: common.12,
            renderers: common.13,
            program_evidence_lane: common.14,
            program_build_receipt: common.15,
            rom_class: common.16,
            characterization_fixture: if characterization {
                "ready_for_controlled_native_evidence"
            } else {
                "not_requested"
            }
            .to_owned(),
            characterization_suite: if characterization {
                F3DZEX2_CHARACTERIZATION_SUITE
            } else {
                "not_requested"
            }
            .to_owned(),
            characterization_vector_source: if characterization {
                "repository_generated"
            } else {
                "not_requested"
            }
            .to_owned(),
            required_characterization_cases: if characterization {
                F3DZEX2_CHARACTERIZATION_CASES
                    .iter()
                    .map(|value| (*value).to_owned())
                    .collect()
            } else {
                Vec::new()
            },
        }))
    } else {
        Ok(StoredReadiness::V5(ReadinessV5 {
            status: common.0,
            purpose: common.1,
            wire_family: common.2,
            report_scenario: common.3,
            artifact_roles_admitted: common.4,
            extended_gbi_fixture: common.5,
            full_rom_inputs: common.6,
            release_matrix_policy: common.7,
            repeat_bar: common.8,
            required_extended_cases: common.9,
            platform: common.10,
            controllers: common.11,
            save: common.12,
            renderers: common.13,
            program_evidence_lane: common.14,
            program_build_receipt: common.15,
            rom_class: common.16,
        }))
    }
}

fn validate_readiness(readiness: &StoredReadiness) -> Result<(), PrivateInputAdmissionError> {
    match readiness {
        StoredReadiness::V6(value) => validate_readiness_common(
            READINESS_SCHEMA,
            value.status.as_str(),
            value.purpose,
            value.wire_family,
            &value.report_scenario,
            &value.artifact_roles_admitted,
            &value.extended_gbi_fixture,
            &value.full_rom_inputs,
            &value.release_matrix_policy,
            value.repeat_bar,
            &value.required_extended_cases,
            value.platform,
            &value.controllers,
            value.save,
            &value.renderers,
            value.program_evidence_lane,
            &value.program_build_receipt,
            value.rom_class,
            Some((
                &value.characterization_fixture,
                &value.characterization_suite,
                &value.characterization_vector_source,
                &value.required_characterization_cases,
            )),
        ),
        StoredReadiness::V5(value) => validate_readiness_common(
            LEGACY_READINESS_SCHEMA,
            value.status.as_str(),
            value.purpose,
            value.wire_family,
            &value.report_scenario,
            &value.artifact_roles_admitted,
            &value.extended_gbi_fixture,
            &value.full_rom_inputs,
            &value.release_matrix_policy,
            value.repeat_bar,
            &value.required_extended_cases,
            value.platform,
            &value.controllers,
            value.save,
            &value.renderers,
            value.program_evidence_lane,
            &value.program_build_receipt,
            value.rom_class,
            None,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_readiness_common(
    schema: &str,
    status: &str,
    purpose: Purpose,
    wire_family: WireFamily,
    scenario: &str,
    roles: &[ArtifactRole],
    extended_fixture: &str,
    full_rom_inputs: &str,
    release_matrix_policy: &str,
    repeat_bar: u64,
    extended_cases: &[String],
    _platform: Platform,
    controllers: &[Controller],
    _save: SavePolicy,
    renderers: &[Renderer],
    program_lane: ProgramEvidenceLane,
    program_receipt: &str,
    rom_class: ManifestRomClass,
    characterization: Option<(&String, &String, &String, &Vec<String>)>,
) -> Result<(), PrivateInputAdmissionError> {
    if status != "ready"
        || release_matrix_policy != "ready_for_ten_run_evidence"
        || repeat_bar != REPEAT_BAR
    {
        return Err(error("readiness fixed policy fields are invalid"));
    }
    if schema == LEGACY_READINESS_SCHEMA
        && (purpose == Purpose::F3dzex2Characterization || wire_family == WireFamily::F3dzex2)
    {
        return Err(error(
            "retained v5 readiness cannot claim F3DZEX2 characterization",
        ));
    }
    validate_scenario(scenario, "readiness.report_scenario")?;
    validate_unique(roles, "readiness.artifact_roles_admitted")?;
    validate_unique(controllers, "readiness.controllers")?;
    validate_unique(renderers, "readiness.renderers")?;
    if controllers.is_empty() || renderers.is_empty() {
        return Err(error("readiness controllers/renderers must not be empty"));
    }
    let role_set = roles.iter().copied().collect::<BTreeSet<_>>();
    if purpose == Purpose::F3dzex2Characterization {
        if role_set
            != BTreeSet::from([
                ArtifactRole::MicrocodeDataRawWindow,
                ArtifactRole::MicrocodeTextRawWindow,
            ])
        {
            return Err(error(
                "readiness F3DZEX2 characterization roles are incomplete or ambiguous",
            ));
        }
    } else if !(role_set.contains(&ArtifactRole::MicrocodeData)
        && role_set.contains(&ArtifactRole::MicrocodeText))
        || role_set.contains(&ArtifactRole::MicrocodeDataRawWindow)
        || role_set.contains(&ArtifactRole::MicrocodeTextRawWindow)
    {
        return Err(error("readiness logical microcode roles are invalid"));
    }
    let renderer_set = renderers.iter().copied().collect::<BTreeSet<_>>();
    if renderer_set.contains(&Renderer::ReferenceLleAccuracy) {
        if renderer_set != BTreeSet::from([Renderer::ReferenceLleAccuracy]) {
            return Err(error("readiness reference LLE must stand alone"));
        }
    } else if !renderer_set.contains(&Renderer::Rt64LleAccuracy) {
        return Err(error("readiness RT64 policy lacks rt64_lle_accuracy"));
    }
    validate_unique_strings(extended_cases, "readiness.required_extended_cases")?;
    let extended_set = extended_cases
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let expected_extended = EXTENDED_CASES.into_iter().collect::<BTreeSet<_>>();
    if purpose.requests_extended_gbi() {
        if extended_fixture != "ready_for_runtime_recognition"
            || extended_set != expected_extended
            || !(renderer_set.contains(&Renderer::Rt64LleAccuracy)
                && renderer_set.contains(&Renderer::Rt64PostViCapture))
        {
            return Err(error("readiness Extended GBI state is inconsistent"));
        }
    } else if extended_fixture != "not_requested" || !extended_set.is_empty() {
        return Err(error("readiness claims unrequested Extended GBI"));
    }
    if !matches!(full_rom_inputs, "ready" | "not_supplied") {
        return Err(error("readiness full_rom_inputs is invalid"));
    }
    if purpose.is_private_run() {
        if rom_class == ManifestRomClass::NotApplicable
            || full_rom_inputs != "ready"
            || !(role_set.contains(&ArtifactRole::Rom)
                && role_set.contains(&ArtifactRole::Recompiled))
            || program_receipt != "verified"
            || !program_lane.is_authoritative()
        {
            return Err(error("readiness full-ROM policy is incomplete"));
        }
    } else if rom_class != ManifestRomClass::NotApplicable
        || program_receipt != "not_applicable"
        || program_lane != ProgramEvidenceLane::NoProgramFixture
    {
        return Err(error("readiness fixture program policy is inconsistent"));
    }
    if purpose == Purpose::F3dzex2Characterization {
        let Some((fixture, suite, source, cases)) = characterization else {
            return Err(error(
                "F3DZEX2 characterization requires current readiness schema",
            ));
        };
        if fixture != "ready_for_controlled_native_evidence"
            || suite != F3DZEX2_CHARACTERIZATION_SUITE
            || source != "repository_generated"
            || cases.iter().map(String::as_str).collect::<Vec<_>>()
                != F3DZEX2_CHARACTERIZATION_CASES
        {
            return Err(error(
                "readiness F3DZEX2 characterization suite contract is incomplete",
            ));
        }
    } else if let Some((fixture, suite, source, cases)) = characterization {
        if fixture != "not_requested"
            || suite != "not_requested"
            || source != "not_requested"
            || !cases.is_empty()
        {
            return Err(error(
                "readiness claims unrequested F3DZEX2 characterization",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
fn build_private_run_contract(
    manifest: &ValidatedManifest,
    manifest_measurement: &StableFileMeasurement,
    readiness_path: &Path,
    readiness_bytes: &[u8],
) -> Result<PrivateReleaseRunContract, PrivateInputAdmissionError> {
    if !manifest.purpose.is_private_run() {
        return Err(error(
            "private run-contract emission requires purpose full_rom or combined",
        ));
    }
    let receipt = manifest
        .program_receipt
        .as_ref()
        .ok_or_else(|| error("private run contract requires a verified program-build receipt"))?;
    let rom = manifest
        .artifacts
        .get(&ArtifactRole::Rom)
        .ok_or_else(|| error("private run contract requires an admitted ROM input"))?;
    let mut artifacts = manifest
        .artifacts
        .iter()
        .filter(|(role, _)| **role != ArtifactRole::Rom)
        .map(|(role, artifact)| private_artifact_identity(*role, artifact))
        .collect::<Result<Vec<_>, _>>()?;
    artifacts.sort_unstable_by(|left, right| left.role.cmp(&right.role));
    let environment = manifest
        .runner
        .env
        .0
        .iter()
        .map(|(name, value)| PrivateEnvironmentEntry {
            name: name.clone(),
            value: value.clone(),
        })
        .collect();
    Ok(PrivateReleaseRunContract {
        schema: PRIVATE_RELEASE_RUN_CONTRACT_SCHEMA.to_owned(),
        admission_manifest: private_file_identity(manifest_measurement)?,
        readiness_report: PrivateFileIdentity {
            path: path_to_utf8(readiness_path, "readiness output")?,
            bytes: u64::try_from(readiness_bytes.len())
                .map_err(|_| error("readiness payload length exceeds u64"))?,
            sha256: sha256_hex(readiness_bytes),
        },
        program_build_receipt: Some(private_file_identity(receipt)?),
        purpose: manifest.purpose.wire_name().to_owned(),
        rom_class: manifest.rom_class.release_class()?,
        report_scenario: manifest.report_scenario.clone(),
        guest_cycle: manifest.runner.release_gate_cycle,
        repeat_count: usize::try_from(REPEAT_BAR).expect("repeat bar fits usize"),
        input: private_artifact_identity(ArtifactRole::Rom, rom)?,
        admitted_artifacts: artifacts,
        expected_execution_source: manifest.runner.execution_source.clone(),
        child: PrivateChildCommand {
            executable: private_file_identity(&manifest.executable)?,
            working_directory: manifest.runner.working_directory.clone(),
            argv: manifest.runner.argv.clone(),
            environment,
        },
        contract_sha256: String::new(),
    })
}

fn cross_bind_contract(
    repository: &PrivateRepository,
    contract: &PrivateReleaseRunContract,
    manifest: &ValidatedManifest,
) -> Result<(), PrivateInputAdmissionError> {
    if contract.purpose != manifest.purpose.wire_name()
        || contract.rom_class != manifest.rom_class.release_class()?
        || contract.report_scenario != manifest.report_scenario
        || contract.guest_cycle != manifest.runner.release_gate_cycle
        || contract.repeat_count != usize::try_from(REPEAT_BAR).expect("repeat bar fits usize")
        || contract.expected_execution_source != manifest.runner.execution_source
    {
        return Err(error(
            "contract policy fields do not match the validated manifest",
        ));
    }
    if matches!(
        contract.expected_execution_source,
        ExecutionDestinationSource::NoProgram
    ) {
        return Err(error(
            "contract execution source does not match an authoritative manifest lane",
        ));
    }

    let receipt_identity = contract
        .program_build_receipt
        .as_ref()
        .ok_or_else(|| error("private run contract omits program_build_receipt"))?;
    let manifest_receipt = manifest
        .program_receipt
        .as_ref()
        .ok_or_else(|| error("validated manifest omits program_build_receipt"))?;
    require_private_identity_matches(
        receipt_identity,
        manifest_receipt,
        "contract.program_build_receipt",
    )?;

    let rom = manifest
        .artifacts
        .get(&ArtifactRole::Rom)
        .ok_or_else(|| error("validated manifest omits ROM"))?;
    require_contract_artifact_matches(&contract.input, ArtifactRole::Rom, rom, "contract.input")?;
    if contract.input.provenance
        != manifest
            .rom_class
            .expected_rom_provenance()
            .ok_or_else(|| error("private contract ROM class is invalid"))?
    {
        return Err(error(
            "private run contract ROM provenance does not match its class",
        ));
    }

    let expected_roles = manifest
        .artifacts
        .keys()
        .copied()
        .filter(|role| *role != ArtifactRole::Rom)
        .map(ArtifactRole::wire_name)
        .collect::<BTreeSet<_>>();
    let observed_roles = contract
        .admitted_artifacts
        .iter()
        .map(|artifact| artifact.role.as_str())
        .collect::<Vec<_>>();
    if !observed_roles.windows(2).all(|pair| pair[0] < pair[1])
        || observed_roles.iter().copied().collect::<BTreeSet<_>>() != expected_roles
    {
        return Err(error(
            "contract admitted artifact roles are not the exact sorted manifest roles",
        ));
    }
    for artifact in &contract.admitted_artifacts {
        let role = parse_artifact_role(&artifact.role)?;
        let expected = manifest.artifacts.get(&role).ok_or_else(|| {
            error(format!(
                "contract artifact {:?} is not admitted",
                artifact.role
            ))
        })?;
        require_contract_artifact_matches(
            artifact,
            role,
            expected,
            &format!("contract artifact {:?}", artifact.role),
        )?;
    }

    require_private_identity_matches(
        &contract.child.executable,
        &manifest.executable,
        "contract.child.executable",
    )?;
    let working = Path::new(&contract.child.working_directory);
    map_fs(
        check_directory_nofollow(working, "contract.child.working_directory"),
        "inspect contract.child.working_directory",
    )?;
    map_fs(
        repository.require_outside_or_gitignored(working, "contract.child.working_directory"),
        "exclude contract.child.working_directory from git",
    )?;
    let names = contract
        .child
        .environment
        .iter()
        .map(|entry| entry.name.as_str())
        .collect::<Vec<_>>();
    if !names.windows(2).all(|pair| pair[0] < pair[1]) {
        return Err(error(
            "contract child environment is not strictly sorted and unique",
        ));
    }
    let environment = contract
        .child
        .environment
        .iter()
        .map(|entry| (entry.name.clone(), entry.value.clone()))
        .collect::<BTreeMap<_, _>>();
    if contract.child.working_directory != manifest.runner.working_directory
        || contract.child.argv != manifest.runner.argv
        || environment != manifest.runner.env.0
    {
        return Err(error(
            "contract child policy does not match the manifest runner",
        ));
    }
    Ok(())
}

fn read_bound_descriptor(
    repository: &PrivateRepository,
    identity: &PrivateFileIdentity,
    field: &str,
) -> Result<StableFileRead, PrivateInputAdmissionError> {
    validate_positive_length(identity.bytes, &format!("{field}.bytes"))?;
    require_sha256(&identity.sha256, &format!("{field}.sha256"))?;
    let read = read_private_file(repository, Path::new(&identity.path), field)?;
    require_measurement(&read.measurement, identity.bytes, &identity.sha256, field)?;
    Ok(read)
}

#[cfg(test)]
fn validate_private_output_path(
    repository: &PrivateRepository,
    path: &Path,
    field: &str,
) -> Result<(), PrivateInputAdmissionError> {
    path_to_utf8(path, field)?;
    map_fs(
        validate_absolute_no_parent(path, field),
        &format!("validate {field}"),
    )?;
    map_fs(
        repository.require_outside_or_gitignored(path, field),
        &format!("exclude {field} from git"),
    )
}

fn read_private_file(
    repository: &PrivateRepository,
    path: &Path,
    field: &str,
) -> Result<StableFileRead, PrivateInputAdmissionError> {
    path_to_utf8(path, field)?;
    map_fs(
        validate_absolute_no_parent(path, field),
        &format!("validate {field}"),
    )?;
    map_fs(
        repository.require_outside_or_gitignored(path, field),
        &format!("exclude {field} from git"),
    )?;
    map_fs(read_regular_stable(path, field), &format!("read {field}"))
}

fn measure_private_file(
    repository: &PrivateRepository,
    path: &Path,
    field: &str,
) -> Result<StableFileMeasurement, PrivateInputAdmissionError> {
    path_to_utf8(path, field)?;
    map_fs(
        validate_absolute_no_parent(path, field),
        &format!("validate {field}"),
    )?;
    map_fs(
        repository.require_outside_or_gitignored(path, field),
        &format!("exclude {field} from git"),
    )?;
    map_fs(
        measure_regular_stable(path, field),
        &format!("measure {field}"),
    )
}

fn require_measurement(
    measurement: &StableFileMeasurement,
    expected_bytes: u64,
    expected_sha256: &str,
    field: &str,
) -> Result<(), PrivateInputAdmissionError> {
    if measurement.bytes != expected_bytes || measurement.sha256 != expected_sha256 {
        return Err(error(format!(
            "{field} identity drift: expected bytes={expected_bytes} sha256={expected_sha256}, observed bytes={} sha256={}",
            measurement.bytes, measurement.sha256
        )));
    }
    Ok(())
}

fn measure_private_executable(
    repository: &PrivateRepository,
    path: &Path,
    field: &str,
) -> Result<StableFileMeasurement, PrivateInputAdmissionError> {
    path_to_utf8(path, field)?;
    map_fs(
        validate_absolute_no_parent(path, field),
        &format!("validate {field}"),
    )?;
    map_fs(
        repository.require_outside_or_gitignored(path, field),
        &format!("exclude {field} from git"),
    )?;

    let mut prefix = [0u8; 64];
    let mut prefix_length = 0usize;
    let mut stream_offset = 0u64;
    let mut pe_offset = None;
    let mut pe_magic = [0u8; 4];
    let mut pe_magic_length = 0usize;
    let measurement = map_fs(
        measure_regular_stable_with(path, field, |event| {
            let StableFileStream::Chunk(chunk) = event else {
                return Ok(());
            };
            if prefix_length < prefix.len() {
                let count = (prefix.len() - prefix_length).min(chunk.len());
                prefix[prefix_length..prefix_length + count].copy_from_slice(&chunk[..count]);
                prefix_length += count;
                if prefix_length == prefix.len() && prefix.starts_with(b"MZ") {
                    pe_offset = Some(u64::from(u32::from_le_bytes(
                        prefix[0x3c..0x40]
                            .try_into()
                            .expect("fixed executable-prefix slice"),
                    )));
                    let target = usize::try_from(pe_offset.expect("PE offset was just set"))
                        .unwrap_or(usize::MAX);
                    if let Some(header) = prefix.get(target..target.saturating_add(4)) {
                        pe_magic.copy_from_slice(header);
                        pe_magic_length = 4;
                    }
                }
            }
            if let Some(target) = pe_offset {
                let chunk_end = stream_offset
                    .checked_add(u64::try_from(chunk.len()).expect("bounded chunk length fits u64"))
                    .ok_or_else(|| format!("{field} stream offset overflow"))?;
                let target_end = target.saturating_add(4);
                let overlap_start = target.max(stream_offset);
                let overlap_end = target_end.min(chunk_end);
                if overlap_start < overlap_end {
                    let source_start = usize::try_from(overlap_start - stream_offset)
                        .expect("overlap lies within bounded chunk");
                    let destination_start = usize::try_from(overlap_start - target)
                        .expect("four-byte destination offset fits usize");
                    let count = usize::try_from(overlap_end - overlap_start)
                        .expect("four-byte overlap fits usize");
                    pe_magic[destination_start..destination_start + count]
                        .copy_from_slice(&chunk[source_start..source_start + count]);
                    pe_magic_length = pe_magic_length.max(destination_start + count);
                }
            }
            stream_offset = stream_offset
                .checked_add(u64::try_from(chunk.len()).expect("bounded chunk length fits u64"))
                .ok_or_else(|| format!("{field} stream offset overflow"))?;
            Ok(())
        }),
        &format!("measure {field}"),
    )?;

    let magic = prefix.get(..prefix_length.min(4)).unwrap_or_default();
    let elf = magic == b"\x7fELF";
    let mach_o = magic.get(..4).is_some_and(|magic| {
        matches!(
            magic,
            [0xfe, 0xed, 0xfa, 0xce]
                | [0xce, 0xfa, 0xed, 0xfe]
                | [0xfe, 0xed, 0xfa, 0xcf]
                | [0xcf, 0xfa, 0xed, 0xfe]
                | [0xca, 0xfe, 0xba, 0xbe]
                | [0xbe, 0xba, 0xfe, 0xca]
                | [0xca, 0xfe, 0xba, 0xbf]
                | [0xbf, 0xba, 0xfe, 0xca]
        )
    });
    let pe = prefix.starts_with(b"MZ") && pe_magic_length == 4 && pe_magic == *b"PE\0\0";
    if !(elf || mach_o || pe) {
        return Err(error(format!(
            "{field} must be a native ELF, Mach-O, or PE image; scripts are forbidden"
        )));
    }
    #[cfg(unix)]
    if rustix::fs::access(path, rustix::fs::Access::EXEC_OK).is_err() {
        return Err(error(format!(
            "{field} has native image bytes but is not executable by the current process"
        )));
    }
    Ok(measurement)
}

fn validate_executable_descriptor(
    descriptor: &ExecutableDescriptor,
    field: &str,
) -> Result<(), PrivateInputAdmissionError> {
    if descriptor.git_identity != "excluded" {
        return Err(error(format!("{field}.git_identity must be 'excluded'")));
    }
    validate_positive_length(descriptor.length, &format!("{field}.length"))?;
    require_sha256(&descriptor.sha256, &format!("{field}.sha256"))
}

fn validate_positive_length(value: u64, field: &str) -> Result<(), PrivateInputAdmissionError> {
    if value == 0 || value > MAX_ARTIFACT_BYTES {
        return Err(error(format!(
            "{field} must be positive and at most {MAX_ARTIFACT_BYTES} bytes"
        )));
    }
    Ok(())
}

fn validate_execution_source(
    source: &ExecutionDestinationSource,
    lane: ProgramEvidenceLane,
    field: &str,
) -> Result<(), PrivateInputAdmissionError> {
    let observed_kind = match source {
        ExecutionDestinationSource::NoProgram => "no_program",
        ExecutionDestinationSource::NativeArchive { artifact_sha256 }
        | ExecutionDestinationSource::TypedObservedFunctionProgram { artifact_sha256 } => {
            require_sha256(artifact_sha256, &format!("{field}.artifact_sha256"))?;
            match source {
                ExecutionDestinationSource::NativeArchive { .. } => "native_archive",
                _ => "typed_observed_function_program",
            }
        }
        ExecutionDestinationSource::TypedBlockProgram {
            program_sha256,
            dispatch_artifact_sha256,
        } => {
            require_sha256(program_sha256, &format!("{field}.program_sha256"))?;
            require_sha256(
                dispatch_artifact_sha256,
                &format!("{field}.dispatch_artifact_sha256"),
            )?;
            "typed_block_program"
        }
    };
    if observed_kind != lane.execution_kind() {
        return Err(error(format!(
            "{field}.kind {observed_kind:?} does not match program lane {lane:?}"
        )));
    }
    Ok(())
}

fn validate_environment_name(name: &str) -> Result<(), PrivateInputAdmissionError> {
    let bytes = name.as_bytes();
    let valid = !bytes.is_empty()
        && (bytes[0].is_ascii_uppercase() || bytes[0] == b'_')
        && bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || *byte == b'_');
    if !valid {
        return Err(error(format!("runner.env name {name:?} is invalid")));
    }
    if RESERVED_RUNNER_ENV.contains(&name)
        || name.starts_with("FN64_RELEASE_")
        || name.starts_with("FN64_PRIVATE_RUN_")
        || name.starts_with("OOT_RELEASE_")
    {
        return Err(error(format!(
            "runner.env name {name:?} is reserved for the trusted runner"
        )));
    }
    if FORBIDDEN_RUNNER_ENV.contains(&name)
        || FORBIDDEN_RUNNER_ENV_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
    {
        return Err(error(format!(
            "runner.env name {name:?} can inject or replace child process code"
        )));
    }
    Ok(())
}

fn validate_scenario(value: &str, field: &str) -> Result<(), PrivateInputAdmissionError> {
    let bytes = value.as_bytes();
    let canonical = (1..=128).contains(&bytes.len())
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes.iter().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(*byte, b'.' | b'_' | b'-')
        });
    let looks_like_sha256 = bytes.len() == 64
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte));
    if !canonical || looks_like_sha256 {
        return Err(error(format!("{field} is invalid")));
    }
    Ok(())
}

fn validate_unique<T: Ord + Copy>(
    values: &[T],
    field: &str,
) -> Result<(), PrivateInputAdmissionError> {
    if values.iter().copied().collect::<BTreeSet<_>>().len() != values.len() {
        return Err(error(format!("{field} contains duplicates")));
    }
    Ok(())
}

fn validate_unique_strings(
    values: &[String],
    field: &str,
) -> Result<(), PrivateInputAdmissionError> {
    if values.iter().any(String::is_empty)
        || values
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
            .len()
            != values.len()
    {
        return Err(error(format!(
            "{field} entries must be nonempty and unique"
        )));
    }
    Ok(())
}

fn require_sha256(value: &str, field: &str) -> Result<(), PrivateInputAdmissionError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(error(format!(
            "{field} must be a lowercase hexadecimal SHA-256"
        )));
    }
    Ok(())
}

#[cfg(test)]
fn private_file_identity(
    measurement: &StableFileMeasurement,
) -> Result<PrivateFileIdentity, PrivateInputAdmissionError> {
    Ok(PrivateFileIdentity {
        path: path_to_utf8(&measurement.path, "private file identity")?,
        bytes: measurement.bytes,
        sha256: measurement.sha256.clone(),
    })
}

#[cfg(test)]
fn private_artifact_identity(
    role: ArtifactRole,
    artifact: &AdmittedArtifact,
) -> Result<PrivateArtifactIdentity, PrivateInputAdmissionError> {
    Ok(PrivateArtifactIdentity {
        role: role.wire_name().to_owned(),
        path: path_to_utf8(&artifact.measurement.path, "private artifact identity")?,
        bytes: artifact.measurement.bytes,
        sha256: artifact.measurement.sha256.clone(),
        provenance: artifact.descriptor.provenance.clone(),
    })
}

fn require_private_identity_matches(
    identity: &PrivateFileIdentity,
    measurement: &StableFileMeasurement,
    field: &str,
) -> Result<(), PrivateInputAdmissionError> {
    if !same_lexical_path(Path::new(&identity.path), &measurement.path)
        || identity.bytes != measurement.bytes
        || identity.sha256 != measurement.sha256
    {
        return Err(error(format!(
            "{field} does not match the validated manifest"
        )));
    }
    Ok(())
}

fn require_contract_artifact_matches(
    identity: &PrivateArtifactIdentity,
    role: ArtifactRole,
    artifact: &AdmittedArtifact,
    field: &str,
) -> Result<(), PrivateInputAdmissionError> {
    if identity.role != role.wire_name()
        || !same_lexical_path(Path::new(&identity.path), &artifact.measurement.path)
        || identity.bytes != artifact.measurement.bytes
        || identity.sha256 != artifact.measurement.sha256
        || identity.provenance != artifact.descriptor.provenance
    {
        return Err(error(format!(
            "{field} does not match the admitted manifest descriptor"
        )));
    }
    Ok(())
}

fn parse_artifact_role(value: &str) -> Result<ArtifactRole, PrivateInputAdmissionError> {
    match value {
        "microcode_data" => Ok(ArtifactRole::MicrocodeData),
        "microcode_data_raw_window" => Ok(ArtifactRole::MicrocodeDataRawWindow),
        "microcode_text" => Ok(ArtifactRole::MicrocodeText),
        "microcode_text_raw_window" => Ok(ArtifactRole::MicrocodeTextRawWindow),
        "recompiled" => Ok(ArtifactRole::Recompiled),
        "rom" => Ok(ArtifactRole::Rom),
        _ => Err(error(format!(
            "contract artifact role {value:?} is invalid"
        ))),
    }
}

fn serialize_json_document<T: Serialize>(
    value: &T,
    field: &str,
) -> Result<Vec<u8>, PrivateInputAdmissionError> {
    let utf8 = serde_json::to_string_pretty(value)
        .map_err(|source| error(format!("serialize {field}: {source}")))?;
    // Python's retained canonical writer uses `json.dumps(..., indent=2)`
    // with its default `ensure_ascii=True`. Preserve that byte wire for paths
    // and environment values outside printable ASCII, including surrogate
    // pairs for non-BMP code points.
    let mut bytes = Vec::with_capacity(utf8.len());
    for character in utf8.chars() {
        if character <= '~' {
            let mut encoded = [0u8; 4];
            bytes.extend_from_slice(character.encode_utf8(&mut encoded).as_bytes());
        } else {
            let scalar = u32::from(character);
            if scalar <= 0xffff {
                bytes.extend_from_slice(format!("\\u{scalar:04x}").as_bytes());
            } else {
                let adjusted = scalar - 0x1_0000;
                let high = 0xd800 | (adjusted >> 10);
                let low = 0xdc00 | (adjusted & 0x3ff);
                bytes.extend_from_slice(format!("\\u{high:04x}\\u{low:04x}").as_bytes());
            }
        }
    }
    bytes.push(b'\n');
    Ok(bytes)
}

fn path_to_utf8(path: &Path, field: &str) -> Result<String, PrivateInputAdmissionError> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| error(format!("{field} path must be valid UTF-8")))
}

#[cfg(test)]
fn sha256_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(bytes);
    let mut value = String::with_capacity(64);
    for byte in digest {
        value.push(char::from(DIGITS[usize::from(byte >> 4)]));
        value.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    struct TempDirectory(PathBuf);

    impl TempDirectory {
        fn new(label: &str) -> Self {
            let root = std::env::temp_dir()
                .canonicalize()
                .expect("resolve test temp root");
            let path = root.join(format!(
                "fn64-private-admission-{label}-{}-{}",
                std::process::id(),
                NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).expect("create private admission test directory");
            Self(path)
        }
    }

    impl Drop for TempDirectory {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).expect("remove private admission test directory");
        }
    }

    mod corpus {
        include!("private_input_admission_corpus_tests.rs");
    }

    #[test]
    fn strict_environment_rejects_duplicate_names() {
        let error = serde_json::from_str::<StrictEnvironment>(r#"{"SAFE":"a","SAFE":"b"}"#)
            .unwrap_err()
            .to_string();
        assert!(error.contains("duplicate field \"SAFE\""), "{error}");
    }

    #[test]
    fn exact_integer_fields_reject_json_booleans() {
        let descriptor = r#"{
            "path":"/private/input",
            "length":true,
            "sha256":"0000000000000000000000000000000000000000000000000000000000000000",
            "provenance":"user_owned_rom_derived",
            "git_identity":"excluded"
        }"#;
        assert!(serde_json::from_str::<ArtifactDescriptor>(descriptor).is_err());
    }

    #[test]
    fn retained_v6_shape_rejects_current_characterization_field() {
        let document = format!(
            r#"{{
              "schema":"{LEGACY_MANIFEST_SCHEMA}",
              "purpose":"extended_gbi",
              "intent":{{
                "wire_family":"f3dex2_extended_gbi_v1",
                "report_scenario":"fixture",
                "recognition":"runtime_must_confirm_backend_known_pair",
                "extended_gbi_cases":[],
                "program_evidence_lane":"no_program_fixture",
                "rom_class":"not_applicable",
                "characterization_suite":null
              }},
              "release_matrix":{{
                "platform":"macos_arm64",
                "controllers":["standard_controller"],
                "save":"no_cartridge_save",
                "renderers":["reference_lle_accuracy"],
                "repeat_bar":10
              }},
              "artifacts":{{
                "microcode_text":null,
                "microcode_data":null,
                "rom":null,
                "recompiled":null
              }},
              "runner":{{
                "executable":{{"path":"/x","length":1,"sha256":"{}","git_identity":"excluded"}},
                "working_directory":"/x",
                "argv":[],"env":{{}},"release_gate_cycle":0,
                "execution_source":{{"kind":"no_program"}},
                "program_build_receipt":null
              }}
            }}"#,
            "0".repeat(64)
        );
        assert!(serde_json::from_str::<StoredManifest>(&document).is_err());
    }

    #[test]
    fn scenario_rejects_digest_shaped_identity() {
        assert!(validate_scenario(&"a".repeat(64), "scenario").is_err());
        assert!(validate_scenario("representative.full-rom-1", "scenario").is_ok());
    }

    #[test]
    fn environment_policy_rejects_loader_and_trusted_runner_names() {
        for name in ["LD_PRELOAD", "DYLD_INSERT_LIBRARIES", "FN64_RELEASE_REPORT"] {
            assert!(validate_environment_name(name).is_err(), "accepted {name}");
        }
        assert!(validate_environment_name("FN64_RENDER").is_ok());
    }

    #[test]
    fn readiness_serialization_preserves_python_field_order_and_vocab() {
        let readiness = StoredReadiness::V6(ReadinessV6 {
            status: "ready".to_owned(),
            purpose: Purpose::FullRom,
            wire_family: WireFamily::FullRomMixed,
            report_scenario: "representative.full-rom-1".to_owned(),
            rom_class: ManifestRomClass::RetailCartridge,
            program_evidence_lane: ProgramEvidenceLane::TypedBlockProgram,
            artifact_roles_admitted: vec![
                ArtifactRole::MicrocodeData,
                ArtifactRole::MicrocodeText,
                ArtifactRole::Recompiled,
                ArtifactRole::Rom,
            ],
            extended_gbi_fixture: "not_requested".to_owned(),
            full_rom_inputs: "ready".to_owned(),
            program_build_receipt: "verified".to_owned(),
            release_matrix_policy: "ready_for_ten_run_evidence".to_owned(),
            repeat_bar: 10,
            required_extended_cases: Vec::new(),
            platform: Platform::WindowsX86_64,
            controllers: vec![Controller::Standard],
            save: SavePolicy::Eeprom4Kbit,
            renderers: vec![Renderer::Rt64LleAccuracy],
            characterization_fixture: "not_requested".to_owned(),
            characterization_suite: "not_requested".to_owned(),
            characterization_vector_source: "not_requested".to_owned(),
            required_characterization_cases: Vec::new(),
        });
        validate_readiness(&readiness).expect("representative readiness is valid");
        let payload = String::from_utf8(
            serialize_json_document(&readiness, "readiness").expect("serialize readiness"),
        )
        .expect("readiness JSON is UTF-8");
        let ordered_fields = [
            "\"schema\"",
            "\"status\"",
            "\"purpose\"",
            "\"wire_family\"",
            "\"report_scenario\"",
            "\"rom_class\"",
            "\"program_evidence_lane\"",
            "\"artifact_roles_admitted\"",
            "\"extended_gbi_fixture\"",
            "\"full_rom_inputs\"",
            "\"program_build_receipt\"",
            "\"release_matrix_policy\"",
        ];
        let mut previous = 0;
        for field in ordered_fields {
            let position = payload.find(field).expect("readiness field is present");
            assert!(position >= previous, "{field} is out of canonical order");
            previous = position;
        }
        assert!(payload.contains("\"platform\": \"windows_x86_64\""));
        assert!(payload.contains("\"save\": \"eeprom_4_kbit\""));
        assert!(payload.ends_with('\n'));
    }

    #[test]
    fn duplicate_fields_are_rejected_beyond_environment_maps() {
        let duplicate = r#"{
            "path":"/private/input",
            "path":"/private/replacement",
            "length":1,
            "sha256":"0000000000000000000000000000000000000000000000000000000000000000",
            "provenance":"user_owned_rom_derived",
            "git_identity":"excluded"
        }"#;
        assert!(serde_json::from_str::<ArtifactDescriptor>(duplicate).is_err());
    }

    #[test]
    fn option_shaped_manifest_fields_must_be_present_even_when_null() {
        let intent_with_null = r#"{
            "wire_family":"f3dex2_extended_gbi_v1",
            "report_scenario":"fixture",
            "recognition":"runtime_must_confirm_backend_known_pair",
            "extended_gbi_cases":[],
            "program_evidence_lane":"no_program_fixture",
            "rom_class":"not_applicable",
            "characterization_suite":null
        }"#;
        assert!(serde_json::from_str::<IntentV7>(intent_with_null).is_ok());
        assert!(serde_json::from_str::<IntentV7>(
            &intent_with_null.replace(",\n            \"characterization_suite\":null", "")
        )
        .is_err());

        let artifacts_with_nulls = r#"{
            "microcode_text":null,
            "microcode_data":null,
            "microcode_text_raw_window":null,
            "microcode_data_raw_window":null,
            "rom":null,
            "recompiled":null
        }"#;
        assert!(serde_json::from_str::<ArtifactsV7>(artifacts_with_nulls).is_ok());
        for field in [
            "microcode_text",
            "microcode_data",
            "microcode_text_raw_window",
            "microcode_data_raw_window",
            "rom",
            "recompiled",
        ] {
            let mut value: serde_json::Value =
                serde_json::from_str(artifacts_with_nulls).expect("parse artifact fixture");
            value
                .as_object_mut()
                .expect("artifact fixture is an object")
                .remove(field);
            assert!(
                serde_json::from_value::<ArtifactsV7>(value).is_err(),
                "accepted missing artifacts.{field}"
            );
        }

        let runner_with_null = format!(
            r#"{{
                "executable":{{"path":"/x","length":1,"sha256":"{}","git_identity":"excluded"}},
                "working_directory":"/x",
                "argv":[],
                "env":{{}},
                "release_gate_cycle":0,
                "execution_source":{{"kind":"no_program"}},
                "program_build_receipt":null
            }}"#,
            "0".repeat(64)
        );
        assert!(serde_json::from_str::<RunnerPolicy>(&runner_with_null).is_ok());
        let mut runner: serde_json::Value =
            serde_json::from_str(&runner_with_null).expect("parse runner fixture");
        runner
            .as_object_mut()
            .expect("runner fixture is an object")
            .remove("program_build_receipt");
        assert!(serde_json::from_value::<RunnerPolicy>(runner).is_err());
    }

    #[test]
    fn controller_order_matches_python_lexical_wire_order() {
        let mut controllers = [
            Controller::VoiceRecognitionUnit,
            Controller::Standard,
            Controller::RumblePak,
            Controller::Pak,
            Controller::TransferPak,
        ];
        controllers.sort_unstable_by_key(|controller| controller.wire_name());
        assert_eq!(
            controllers
                .iter()
                .map(|controller| controller.wire_name())
                .collect::<Vec<_>>(),
            [
                "controller_pak",
                "rumble_pak",
                "standard_controller",
                "transfer_pak",
                "voice_recognition_unit",
            ]
        );
    }

    #[test]
    fn native_executable_is_stream_measured_without_retaining_its_image() {
        let repository = PrivateRepository::discover().expect("discover repository");
        let executable = std::env::current_exe()
            .expect("resolve test executable")
            .canonicalize()
            .expect("canonicalize test executable");
        let measured = measure_private_executable(&repository, &executable, "test executable")
            .expect("stream-measure native executable");
        let independently_measured =
            measure_private_file(&repository, &executable, "test executable")
                .expect("independently measure executable");
        assert_eq!(measured, independently_measured);
    }

    #[test]
    fn receipt_bound_paths_share_private_repository_policy() {
        let repository = PrivateRepository::discover().expect("discover repository");
        let tracked = repository.root().join("README.md");
        let tracked_identity = ReleaseProgramFileIdentity {
            path: tracked
                .to_str()
                .expect("repository path is UTF-8")
                .to_owned(),
            bytes: 1,
            sha256: "0".repeat(64),
        };
        let directory = TempDirectory::new("receipt-private-paths");
        let private_identity = ReleaseProgramFileIdentity {
            path: directory
                .0
                .join("private-input")
                .to_str()
                .expect("temporary path is UTF-8")
                .to_owned(),
            bytes: 1,
            sha256: "4".repeat(64),
        };

        let tracked_child = ReleaseProgramBuildReceipt {
            schema: PROGRAM_BUILD_RECEIPT_SCHEMA.to_owned(),
            child_executable: tracked_identity.clone(),
            lane: ReleaseProgramBuildLane::TypedObservedFunction {
                identity_wire: private_identity.clone(),
            },
            expected_execution_source: ExecutionDestinationSource::TypedObservedFunctionProgram {
                artifact_sha256: "2".repeat(64),
            },
            receipt_sha256: "3".repeat(64),
        };
        let rejection = require_receipt_private_paths(&repository, &tracked_child)
            .expect_err("tracked receipt child must be rejected")
            .to_string();
        assert!(rejection.contains("tracked by git"), "{rejection}");

        for lane in [
            ReleaseProgramBuildLane::TypedObservedFunction {
                identity_wire: tracked_identity.clone(),
            },
            ReleaseProgramBuildLane::TypedBlock {
                pack: tracked_identity.clone(),
                expected_program_sha256: "1".repeat(64),
            },
            ReleaseProgramBuildLane::NativeArchives {
                archives: vec![
                    crate::release_program_build_receipt::NativeArchiveBuildInput {
                        label: "tracked".to_owned(),
                        file: tracked_identity.clone(),
                    },
                ],
            },
        ] {
            let receipt = ReleaseProgramBuildReceipt {
                schema: PROGRAM_BUILD_RECEIPT_SCHEMA.to_owned(),
                child_executable: private_identity.clone(),
                lane,
                expected_execution_source:
                    ExecutionDestinationSource::TypedObservedFunctionProgram {
                        artifact_sha256: "2".repeat(64),
                    },
                receipt_sha256: "3".repeat(64),
            };
            let rejection = require_receipt_private_paths(&repository, &receipt)
                .expect_err("tracked receipt path must be rejected")
                .to_string();
            assert!(rejection.contains("tracked by git"), "{rejection}");
        }
    }

    #[test]
    fn canonical_json_matches_python_ensure_ascii_wire() {
        let payload = serialize_json_document(
            &serde_json::json!({"x": "é😀\u{7f}\u{2028}"}),
            "unicode fixture",
        )
        .expect("serialize Unicode fixture");
        assert_eq!(
            payload,
            b"{\n  \"x\": \"\\u00e9\\ud83d\\ude00\\u007f\\u2028\"\n}\n"
        );
    }

    #[test]
    fn current_v7_extended_gbi_admission_derives_exact_readiness_without_python() {
        let temp = TempDirectory::new("current-v7");
        let text_path = temp.0.join("microcode-text.bin");
        let data_path = temp.0.join("microcode-data.bin");
        std::fs::write(&text_path, vec![0x11; 4096]).expect("write synthetic IMEM fixture");
        std::fs::write(&data_path, vec![0x22; 32]).expect("write synthetic data fixture");
        let text_bytes = std::fs::read(&text_path).expect("read synthetic IMEM fixture");
        let data_bytes = std::fs::read(&data_path).expect("read synthetic data fixture");
        let executable = std::env::current_exe()
            .expect("resolve test executable")
            .canonicalize()
            .expect("canonicalize test executable");
        let executable_bytes = std::fs::read(&executable).expect("read test executable");
        let descriptor = |path: &Path, bytes: &[u8], provenance: &str| {
            serde_json::json!({
                "path": path,
                "length": bytes.len(),
                "sha256": sha256_hex(bytes),
                "provenance": provenance,
                "git_identity": "excluded",
            })
        };
        let manifest = serde_json::json!({
            "schema": MANIFEST_SCHEMA,
            "purpose": "extended_gbi",
            "intent": {
                "wire_family": "f3dex2_extended_gbi_v1",
                "report_scenario": "synthetic-extended-gbi",
                "recognition": "runtime_must_confirm_backend_known_pair",
                "extended_gbi_cases": [
                    "hook-control",
                    "disabled-negative-control",
                    "activation",
                    "widescreen",
                    "interpolation",
                    "vertex-z"
                ],
                "program_evidence_lane": "no_program_fixture",
                "rom_class": "not_applicable",
                "characterization_suite": null
            },
            "release_matrix": {
                "platform": "windows_x86_64",
                "controllers": ["standard_controller"],
                "save": "no_cartridge_save",
                "renderers": ["rt64_post_vi_capture", "rt64_lle_accuracy"],
                "repeat_bar": 10
            },
            "artifacts": {
                "microcode_text": descriptor(
                    &text_path,
                    &text_bytes,
                    "user_owned_rom_derived"
                ),
                "microcode_data": descriptor(
                    &data_path,
                    &data_bytes,
                    "user_owned_rom_derived"
                ),
                "microcode_text_raw_window": null,
                "microcode_data_raw_window": null,
                "rom": null,
                "recompiled": null
            },
            "runner": {
                "executable": {
                    "path": &executable,
                    "length": executable_bytes.len(),
                    "sha256": sha256_hex(&executable_bytes),
                    "git_identity": "excluded"
                },
                "working_directory": &temp.0,
                "argv": [],
                "env": {},
                "release_gate_cycle": 0,
                "execution_source": {"kind": "no_program"},
                "program_build_receipt": null
            }
        });
        let manifest_path = temp.0.join("manifest.json");
        std::fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest).expect("serialize synthetic manifest"),
        )
        .expect("write synthetic manifest");
        let readiness_path = temp.0.join("readiness.json");
        let admitted = admit_current_v7_manifest(&manifest_path, &readiness_path)
            .expect("admit synthetic current-v7 manifest");
        assert!(admitted.contract.is_none());
        assert!(admitted.contract_bytes.is_none());
        let expected = b"{\n  \"schema\": \"fn64.private-input-readiness.v6\",\n  \"status\": \"ready\",\n  \"purpose\": \"extended_gbi\",\n  \"wire_family\": \"f3dex2_extended_gbi_v1\",\n  \"report_scenario\": \"synthetic-extended-gbi\",\n  \"rom_class\": \"not_applicable\",\n  \"program_evidence_lane\": \"no_program_fixture\",\n  \"artifact_roles_admitted\": [\n    \"microcode_data\",\n    \"microcode_text\"\n  ],\n  \"extended_gbi_fixture\": \"ready_for_runtime_recognition\",\n  \"full_rom_inputs\": \"not_supplied\",\n  \"program_build_receipt\": \"not_applicable\",\n  \"release_matrix_policy\": \"ready_for_ten_run_evidence\",\n  \"repeat_bar\": 10,\n  \"required_extended_cases\": [\n    \"activation\",\n    \"disabled-negative-control\",\n    \"hook-control\",\n    \"interpolation\",\n    \"vertex-z\",\n    \"widescreen\"\n  ],\n  \"platform\": \"windows_x86_64\",\n  \"controllers\": [\n    \"standard_controller\"\n  ],\n  \"save\": \"no_cartridge_save\",\n  \"renderers\": [\n    \"rt64_lle_accuracy\",\n    \"rt64_post_vi_capture\"\n  ],\n  \"characterization_fixture\": \"not_requested\",\n  \"characterization_suite\": \"not_requested\",\n  \"characterization_vector_source\": \"not_requested\",\n  \"required_characterization_cases\": []\n}\n";
        assert_eq!(admitted.readiness_bytes, expected);
        assert!(
            !readiness_path.exists(),
            "policy must not publish output files"
        );
    }
}
