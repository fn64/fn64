#![allow(clippy::module_inception)]
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Purpose {
    ExtendedGbi,
    F3dzex2Characterization,
    FullRom,
    Combined,
}

impl Purpose {
    pub(super) const fn wire_name(self) -> &'static str {
        match self {
            Self::ExtendedGbi => "extended_gbi",
            Self::F3dzex2Characterization => "f3dzex2_characterization",
            Self::FullRom => "full_rom",
            Self::Combined => "combined",
        }
    }

    pub(super) const fn is_private_run(self) -> bool {
        matches!(self, Self::FullRom | Self::Combined)
    }

    pub(super) const fn requests_extended_gbi(self) -> bool {
        matches!(self, Self::ExtendedGbi | Self::Combined)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum WireFamily {
    F3dex2ExtendedGbiV1,
    F3dex2,
    Fast3dF3dex,
    S2dexS2dex2,
    FullRomMixed,
    F3dzex2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ProgramEvidenceLane {
    NoProgramFixture,
    IdentifiedNativeArchive,
    TypedObservedFunction,
    TypedBlockProgram,
}

impl ProgramEvidenceLane {
    pub(super) const fn execution_kind(self) -> &'static str {
        match self {
            Self::NoProgramFixture => "no_program",
            Self::IdentifiedNativeArchive => "native_archive",
            Self::TypedObservedFunction => "typed_observed_function_program",
            Self::TypedBlockProgram => "typed_block_program",
        }
    }

    pub(super) const fn is_authoritative(self) -> bool {
        !matches!(self, Self::NoProgramFixture)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ManifestRomClass {
    RetailCartridge,
    PublicHomebrew,
    NotApplicable,
}

impl ManifestRomClass {
    pub(super) const fn wire_name(self) -> &'static str {
        match self {
            Self::RetailCartridge => "retail_cartridge",
            Self::PublicHomebrew => "public_homebrew",
            Self::NotApplicable => "not_applicable",
        }
    }

    pub(super) fn release_class(self) -> Result<ReleaseRomClass, PrivateInputAdmissionError> {
        match self {
            Self::RetailCartridge => Ok(ReleaseRomClass::RetailCartridge),
            Self::PublicHomebrew => Ok(ReleaseRomClass::PublicHomebrew),
            Self::NotApplicable => Err(error(
                "private run contract cannot use rom_class='not_applicable'",
            )),
        }
    }

    pub(super) const fn expected_rom_provenance(self) -> Option<&'static str> {
        match self {
            Self::RetailCartridge => Some("user_owned_retail_cartridge_dump"),
            Self::PublicHomebrew => Some("publicly_distributed_homebrew_rom"),
            Self::NotApplicable => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Platform {
    #[serde(rename = "macos_arm64")]
    MacosArm64,
    #[serde(rename = "linux_x86_64")]
    LinuxX86_64,
    #[serde(rename = "windows_x86_64")]
    WindowsX86_64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Controller {
    #[serde(rename = "standard_controller")]
    Standard,
    #[serde(rename = "controller_pak")]
    Pak,
    RumblePak,
    TransferPak,
    VoiceRecognitionUnit,
}

impl Controller {
    pub(super) const fn wire_name(self) -> &'static str {
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
pub(super) enum SavePolicy {
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
pub(super) enum Renderer {
    ReferenceLleAccuracy,
    Rt64LleAccuracy,
    Rt64PostViCapture,
    Rt64ReplacementPacks,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ArtifactRole {
    MicrocodeData,
    MicrocodeDataRawWindow,
    MicrocodeText,
    MicrocodeTextRawWindow,
    Recompiled,
    Rom,
}

impl ArtifactRole {
    pub(super) const fn wire_name(self) -> &'static str {
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
pub(super) struct IntentV7 {
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
pub(super) struct IntentV6 {
    wire_family: WireFamily,
    report_scenario: String,
    recognition: String,
    extended_gbi_cases: Vec<String>,
    program_evidence_lane: ProgramEvidenceLane,
    rom_class: ManifestRomClass,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReleaseMatrixPolicy {
    pub(super) platform: Platform,
    pub(super) controllers: Vec<Controller>,
    pub(super) save: SavePolicy,
    pub(super) renderers: Vec<Renderer>,
    repeat_bar: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ArtifactDescriptor {
    path: String,
    length: u64,
    sha256: String,
    pub(super) provenance: String,
    git_identity: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ExecutableDescriptor {
    path: String,
    pub(super) length: u64,
    pub(super) sha256: String,
    pub(super) git_identity: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ArtifactsV7 {
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
pub(super) struct ArtifactsV6 {
    #[serde(deserialize_with = "deserialize_present_option")]
    microcode_text: Option<ArtifactDescriptor>,
    #[serde(deserialize_with = "deserialize_present_option")]
    microcode_data: Option<ArtifactDescriptor>,
    #[serde(deserialize_with = "deserialize_present_option")]
    rom: Option<ArtifactDescriptor>,
    #[serde(deserialize_with = "deserialize_present_option")]
    recompiled: Option<ArtifactDescriptor>,
}

pub(super) fn deserialize_present_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub(super) struct StrictEnvironment(pub(super) BTreeMap<String, String>);

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
pub(super) struct RunnerPolicy {
    executable: ExecutableDescriptor,
    pub(super) working_directory: String,
    pub(super) argv: Vec<String>,
    pub(super) env: StrictEnvironment,
    pub(super) release_gate_cycle: u64,
    pub(super) execution_source: ExecutionDestinationSource,
    #[serde(deserialize_with = "deserialize_present_option")]
    program_build_receipt: Option<ExecutableDescriptor>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ManifestV7 {
    purpose: Purpose,
    intent: IntentV7,
    release_matrix: ReleaseMatrixPolicy,
    artifacts: ArtifactsV7,
    runner: RunnerPolicy,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ManifestV6 {
    purpose: Purpose,
    intent: IntentV6,
    release_matrix: ReleaseMatrixPolicy,
    artifacts: ArtifactsV6,
    runner: RunnerPolicy,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "schema")]
pub(super) enum StoredManifest {
    #[serde(rename = "fn64.private-input-admission.v7")]
    V7(Box<ManifestV7>),
    #[serde(rename = "fn64.private-input-admission.v6")]
    V6(Box<ManifestV6>),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReadinessV6 {
    pub(super) status: String,
    pub(super) purpose: Purpose,
    pub(super) wire_family: WireFamily,
    pub(super) report_scenario: String,
    pub(super) rom_class: ManifestRomClass,
    pub(super) program_evidence_lane: ProgramEvidenceLane,
    pub(super) artifact_roles_admitted: Vec<ArtifactRole>,
    pub(super) extended_gbi_fixture: String,
    pub(super) full_rom_inputs: String,
    pub(super) program_build_receipt: String,
    pub(super) release_matrix_policy: String,
    pub(super) repeat_bar: u64,
    pub(super) required_extended_cases: Vec<String>,
    pub(super) platform: Platform,
    pub(super) controllers: Vec<Controller>,
    pub(super) save: SavePolicy,
    pub(super) renderers: Vec<Renderer>,
    pub(super) characterization_fixture: String,
    pub(super) characterization_suite: String,
    pub(super) characterization_vector_source: String,
    pub(super) required_characterization_cases: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReadinessV5 {
    pub(super) status: String,
    pub(super) purpose: Purpose,
    pub(super) wire_family: WireFamily,
    pub(super) report_scenario: String,
    pub(super) rom_class: ManifestRomClass,
    pub(super) program_evidence_lane: ProgramEvidenceLane,
    pub(super) artifact_roles_admitted: Vec<ArtifactRole>,
    pub(super) extended_gbi_fixture: String,
    pub(super) full_rom_inputs: String,
    pub(super) program_build_receipt: String,
    pub(super) release_matrix_policy: String,
    pub(super) repeat_bar: u64,
    pub(super) required_extended_cases: Vec<String>,
    pub(super) platform: Platform,
    pub(super) controllers: Vec<Controller>,
    pub(super) save: SavePolicy,
    pub(super) renderers: Vec<Renderer>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "schema")]
pub(super) enum StoredReadiness {
    #[serde(rename = "fn64.private-input-readiness.v6")]
    V6(ReadinessV6),
    #[serde(rename = "fn64.private-input-readiness.v5")]
    V5(ReadinessV5),
}

pub(super) struct AdmittedArtifact {
    pub(super) descriptor: ArtifactDescriptor,
    pub(super) measurement: StableFileMeasurement,
    captured_contents: Option<Vec<u8>>,
}

pub(super) struct ValidatedManifest {
    pub(super) schema: &'static str,
    pub(super) purpose: Purpose,
    pub(super) wire_family: WireFamily,
    pub(super) report_scenario: String,
    pub(super) program_lane: ProgramEvidenceLane,
    pub(super) rom_class: ManifestRomClass,
    pub(super) release: ReleaseMatrixPolicy,
    pub(super) artifacts: BTreeMap<ArtifactRole, AdmittedArtifact>,
    pub(super) runner: RunnerPolicy,
    pub(super) executable: StableFileMeasurement,
    pub(super) program_receipt: Option<StableFileMeasurement>,
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

pub(super) fn validate_current_v7_manifest(
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

pub(super) fn load_private_f3dzex2_characterization_input_inner(
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

pub(super) fn verify_retained_contract_read(
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

pub(super) fn parse_manifest(bytes: &[u8], field: &str) -> Result<StoredManifest, PrivateInputAdmissionError> {
    serde_json::from_slice(bytes).map_err(|source| error(format!("parse {field}: {source}")))
}

pub(super) fn validate_manifest_v7(
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

pub(super) fn validate_manifest_v6(
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
pub(super) fn validate_manifest_common(
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

pub(super) fn validate_release_policy(
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

pub(super) fn validate_artifact(
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

pub(super) fn validate_artifact_denominator(
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

pub(super) fn validate_runner(
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

pub(super) fn require_receipt_private_paths(
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

pub(super) fn require_receipt_private_path(
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

pub(super) fn verify_receipt_binding(
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

pub(super) fn same_identity_with_executable(
    identity: &ReleaseProgramFileIdentity,
    descriptor: &ExecutableDescriptor,
) -> Result<bool, PrivateInputAdmissionError> {
    Ok(
        same_lexical_path(Path::new(&identity.path), Path::new(&descriptor.path))
            && identity.bytes == descriptor.length
            && identity.sha256 == descriptor.sha256,
    )
}

pub(super) fn same_identity_with_artifact(
    identity: &ReleaseProgramFileIdentity,
    artifact: &AdmittedArtifact,
) -> bool {
    identity.path == artifact.descriptor.path
        && identity.bytes == artifact.descriptor.length
        && identity.sha256 == artifact.descriptor.sha256
}
