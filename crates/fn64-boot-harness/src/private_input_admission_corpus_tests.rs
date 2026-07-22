use super::*;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::Command;

const PRIVATE_ADMISSION_CORPUS: &str =
    include_str!("../tests/fixtures/private-admission-corpus-v1.json");
const PRIVATE_ADMISSION_CORPUS_SCHEMA: &str = "fn64.private-admission-rejection-corpus.v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum CorpusBaseline {
    JsonDocument,
    ManifestV7FullRom,
    ManifestV7Characterization,
    ManifestV6Retained,
    ReadinessV5Retained,
    ContractV3FromV7,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum CorpusCapability {
    All,
    Symlink,
    CaseInsensitive,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CorpusExpectation {
    Accept,
    Reject,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum CorpusSemantics {
    V7FullRom,
    V7Characterization,
    V6Retained,
    V3Contract,
    CapturedV3Contract,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum CorpusOperationKind {
    Validate,
    DuplicateRootField,
    DuplicateNestedEnvField,
    SymlinkManifest,
    SymlinkArtifactParent,
    SymlinkWorkingDirectory,
    SameLengthDescriptorSwap,
    SameLengthPathSwap,
    CapturedContractSourceReplacement,
    RelativeArtifactPath,
    ParentArtifactPath,
    TrackedArtifactPath,
    CaseAliasTrackedPath,
    ReservedRomEnv,
    FutureReleaseEnv,
    LdPreloadEnv,
    D3d12sdkPathEnv,
    LowercaseEnv,
    ArtifactTamper,
    ReceiptInnerTamper,
    ReceiptOuterTamper,
    ContractDigestTamper,
    ContractDescriptorTamper,
    V6NewAdmission,
    V6WithV7Field,
    V6F3dzex2RawRole,
    V5WithCurrentField,
}

impl CorpusOperationKind {
    const ALL: [Self; 27] = [
        Self::Validate,
        Self::DuplicateRootField,
        Self::DuplicateNestedEnvField,
        Self::SymlinkManifest,
        Self::SymlinkArtifactParent,
        Self::SymlinkWorkingDirectory,
        Self::SameLengthDescriptorSwap,
        Self::SameLengthPathSwap,
        Self::CapturedContractSourceReplacement,
        Self::RelativeArtifactPath,
        Self::ParentArtifactPath,
        Self::TrackedArtifactPath,
        Self::CaseAliasTrackedPath,
        Self::ReservedRomEnv,
        Self::FutureReleaseEnv,
        Self::LdPreloadEnv,
        Self::D3d12sdkPathEnv,
        Self::LowercaseEnv,
        Self::ArtifactTamper,
        Self::ReceiptInnerTamper,
        Self::ReceiptOuterTamper,
        Self::ContractDigestTamper,
        Self::ContractDescriptorTamper,
        Self::V6NewAdmission,
        Self::V6WithV7Field,
        Self::V6F3dzex2RawRole,
        Self::V5WithCurrentField,
    ];

    const fn wire_name(self) -> &'static str {
        match self {
            Self::Validate => "validate",
            Self::DuplicateRootField => "duplicate-root-field",
            Self::DuplicateNestedEnvField => "duplicate-nested-env-field",
            Self::SymlinkManifest => "symlink-manifest",
            Self::SymlinkArtifactParent => "symlink-artifact-parent",
            Self::SymlinkWorkingDirectory => "symlink-working-directory",
            Self::SameLengthDescriptorSwap => "same-length-descriptor-swap",
            Self::SameLengthPathSwap => "same-length-path-swap",
            Self::CapturedContractSourceReplacement => "captured-contract-source-replacement",
            Self::RelativeArtifactPath => "relative-artifact-path",
            Self::ParentArtifactPath => "parent-artifact-path",
            Self::TrackedArtifactPath => "tracked-artifact-path",
            Self::CaseAliasTrackedPath => "case-alias-tracked-path",
            Self::ReservedRomEnv => "reserved-rom-env",
            Self::FutureReleaseEnv => "future-release-env",
            Self::LdPreloadEnv => "ld-preload-env",
            Self::D3d12sdkPathEnv => "d3d12sdk-path-env",
            Self::LowercaseEnv => "lowercase-env",
            Self::ArtifactTamper => "artifact-tamper",
            Self::ReceiptInnerTamper => "receipt-inner-tamper",
            Self::ReceiptOuterTamper => "receipt-outer-tamper",
            Self::ContractDigestTamper => "contract-digest-tamper",
            Self::ContractDescriptorTamper => "contract-descriptor-tamper",
            Self::V6NewAdmission => "v6-new-admission",
            Self::V6WithV7Field => "v6-with-v7-field",
            Self::V6F3dzex2RawRole => "v6-f3dzex2-raw-role",
            Self::V5WithCurrentField => "v5-with-current-field",
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CorpusOperation {
    kind: CorpusOperationKind,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CorpusRecipe {
    id: String,
    baseline: CorpusBaseline,
    capability: CorpusCapability,
    expect: CorpusExpectation,
    operation: CorpusOperation,
    semantics: Option<CorpusSemantics>,
    rejection_token: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CorpusDocument {
    schema: String,
    recipes: Vec<CorpusRecipe>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CorpusSnapshotResult {
    id: String,
    verdict: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CorpusSnapshotDocument {
    schema: String,
    results: Vec<CorpusSnapshotResult>,
}

struct StrictUniqueObject;

impl<'de> Deserialize<'de> for StrictUniqueObject {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct UniqueObjectVisitor;

        impl<'de> Visitor<'de> for UniqueObjectVisitor {
            type Value = StrictUniqueObject;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a JSON object with unique fields")
            }

            fn visit_map<A>(self, mut access: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut names = BTreeSet::new();
                while let Some(name) = access.next_key::<String>()? {
                    if !names.insert(name.clone()) {
                        return Err(de::Error::custom(format!(
                            "JSON object contains duplicate field {name:?}"
                        )));
                    }
                    access.next_value::<de::IgnoredAny>()?;
                }
                Ok(StrictUniqueObject)
            }
        }

        deserializer.deserialize_map(UniqueObjectVisitor)
    }
}

fn reject_corpus_identity_material(value: &serde_json::Value, where_: &str) -> Result<(), String> {
    match value {
        serde_json::Value::Object(fields) => {
            for (name, item) in fields {
                if matches!(name.as_str(), "path" | "sha256" | "bytes" | "length") {
                    return Err(format!(
                        "{where_} contains forbidden private identity field {name:?}"
                    ));
                }
                reject_corpus_identity_material(item, &format!("{where_}.{name}"))?;
            }
        }
        serde_json::Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                reject_corpus_identity_material(item, &format!("{where_}[{index}]"))?;
            }
        }
        serde_json::Value::String(item) => {
            if Path::new(item).is_absolute() {
                return Err(format!("{where_} contains an absolute path"));
            }
            if item.len() == 64
                && item
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err(format!("{where_} contains a content identity"));
            }
        }
        _ => {}
    }
    Ok(())
}

fn expected_recipe_shape(
    recipe: &CorpusRecipe,
) -> (
    CorpusCapability,
    CorpusBaseline,
    CorpusExpectation,
    Option<CorpusSemantics>,
    String,
    Option<&'static str>,
) {
    use CorpusBaseline as B;
    use CorpusCapability as C;
    use CorpusExpectation as E;
    use CorpusOperationKind as O;
    use CorpusSemantics as S;

    match recipe.operation.kind {
        O::Validate => {
            let (semantics, id) = match recipe.baseline {
                B::ManifestV7FullRom => (S::V7FullRom, "accept-v7-full-rom"),
                B::ManifestV7Characterization => {
                    (S::V7Characterization, "accept-v7-characterization")
                }
                B::ManifestV6Retained => (S::V6Retained, "accept-v6-retained"),
                B::ContractV3FromV7 => (S::V3Contract, "accept-v7-contract"),
                _ => panic!("invalid validate baseline in corpus"),
            };
            (
                C::All,
                recipe.baseline,
                E::Accept,
                Some(semantics),
                id.to_owned(),
                None,
            )
        }
        O::CapturedContractSourceReplacement => (
            C::All,
            B::ContractV3FromV7,
            E::Accept,
            Some(S::CapturedV3Contract),
            "accept-captured-contract-source-replacement".to_owned(),
            None,
        ),
        operation => {
            let (capability, baseline, token) = match operation {
                O::DuplicateRootField => (C::All, B::JsonDocument, "duplicate field 'schema'"),
                O::DuplicateNestedEnvField => (C::All, B::JsonDocument, "duplicate field 'A'"),
                O::SymlinkManifest => (C::Symlink, B::ManifestV7FullRom, "symlink path component"),
                O::SymlinkArtifactParent | O::SymlinkWorkingDirectory => {
                    (C::Symlink, B::ManifestV7FullRom, "symlink path component")
                }
                O::SameLengthDescriptorSwap | O::SameLengthPathSwap => (
                    C::All,
                    B::ManifestV7FullRom,
                    "artifacts.microcode_data SHA-256 drift",
                ),
                O::RelativeArtifactPath => (
                    C::All,
                    B::ManifestV7FullRom,
                    "artifacts.microcode_data.path must be absolute",
                ),
                O::ParentArtifactPath => (
                    C::All,
                    B::ManifestV7FullRom,
                    "artifacts.microcode_data.path must not contain '..'",
                ),
                O::TrackedArtifactPath | O::CaseAliasTrackedPath => (
                    if operation == O::CaseAliasTrackedPath {
                        C::CaseInsensitive
                    } else {
                        C::All
                    },
                    B::ManifestV7FullRom,
                    "inside the repository and not gitignored",
                ),
                O::ReservedRomEnv | O::FutureReleaseEnv => (
                    C::All,
                    B::ManifestV7FullRom,
                    "reserved for the trusted runner",
                ),
                O::LdPreloadEnv | O::D3d12sdkPathEnv => (
                    C::All,
                    B::ManifestV7FullRom,
                    "can inject or replace child process code",
                ),
                O::LowercaseEnv => (
                    C::All,
                    B::ManifestV7FullRom,
                    "runner.env name 'lowercase' is invalid",
                ),
                O::ArtifactTamper => (
                    C::All,
                    B::ManifestV7FullRom,
                    "artifacts.microcode_data length drift",
                ),
                O::ReceiptInnerTamper => (
                    C::All,
                    B::ManifestV7FullRom,
                    "execution source does not match recomputed",
                ),
                O::ReceiptOuterTamper => (
                    C::All,
                    B::ManifestV7FullRom,
                    "runner.program_build_receipt length drift",
                ),
                O::ContractDigestTamper => (
                    C::All,
                    B::ContractV3FromV7,
                    "private run contract SHA-256 drift",
                ),
                O::ContractDescriptorTamper => (
                    C::All,
                    B::ContractV3FromV7,
                    "contract.admission_manifest SHA-256 drift",
                ),
                O::V6NewAdmission => (
                    C::All,
                    B::ManifestV6Retained,
                    "new --manifest admission requires schema",
                ),
                O::V6WithV7Field => (C::All, B::ManifestV6Retained, "intent fields are invalid"),
                O::V6F3dzex2RawRole => (
                    C::All,
                    B::ManifestV6Retained,
                    "unsupported wire family 'f3dzex2'",
                ),
                O::V5WithCurrentField => (
                    C::All,
                    B::ReadinessV5Retained,
                    "readiness report has unknown or missing fields",
                ),
                O::Validate | O::CapturedContractSourceReplacement => unreachable!(),
            };
            (
                capability,
                baseline,
                E::Reject,
                None,
                format!("reject-{}", operation.wire_name()),
                Some(token),
            )
        }
    }
}

fn load_private_admission_corpus() -> CorpusDocument {
    let raw: serde_json::Value =
        serde_json::from_str(PRIVATE_ADMISSION_CORPUS).expect("parse corpus JSON value");
    reject_corpus_identity_material(&raw, "private-admission corpus")
        .expect("corpus contains no private identity material");
    let corpus: CorpusDocument = serde_json::from_str(PRIVATE_ADMISSION_CORPUS)
        .expect("parse strict private-admission corpus");
    assert_eq!(corpus.schema, PRIVATE_ADMISSION_CORPUS_SCHEMA);
    assert!(
        !corpus.recipes.is_empty(),
        "corpus recipes must not be empty"
    );

    let mut ids = BTreeSet::new();
    let mut operations = BTreeSet::new();
    let mut accepted_baselines = BTreeSet::new();
    for recipe in &corpus.recipes {
        validate_scenario(&recipe.id, "private-admission recipe ID")
            .expect("corpus recipe ID is canonical");
        assert!(ids.insert(recipe.id.clone()), "corpus recipe ID repeats");
        operations.insert(recipe.operation.kind);
        let expected = expected_recipe_shape(recipe);
        assert_eq!(recipe.capability, expected.0, "{} capability", recipe.id);
        assert_eq!(recipe.baseline, expected.1, "{} baseline", recipe.id);
        assert_eq!(recipe.expect, expected.2, "{} expectation", recipe.id);
        assert_eq!(recipe.semantics, expected.3, "{} semantics", recipe.id);
        assert_eq!(recipe.id, expected.4, "recipe ID does not match operation");
        assert_eq!(
            recipe.rejection_token.as_deref(),
            expected.5,
            "{} rejection token",
            recipe.id
        );
        if recipe.operation.kind == CorpusOperationKind::Validate {
            accepted_baselines.insert(recipe.baseline);
        }
    }
    assert_eq!(operations, CorpusOperationKind::ALL.into_iter().collect());
    assert_eq!(
        accepted_baselines,
        BTreeSet::from([
            CorpusBaseline::ManifestV7FullRom,
            CorpusBaseline::ManifestV7Characterization,
            CorpusBaseline::ManifestV6Retained,
            CorpusBaseline::ContractV3FromV7,
        ])
    );
    corpus
}

fn write_json_value(path: &Path, value: &serde_json::Value) {
    let mut bytes = serde_json::to_vec_pretty(value).expect("serialize corpus fixture");
    bytes.push(b'\n');
    std::fs::write(path, bytes).expect("write corpus fixture");
}

fn corpus_descriptor(path: &Path, provenance: &str) -> serde_json::Value {
    let bytes = std::fs::read(path).expect("read corpus artifact");
    serde_json::json!({
        "path": path,
        "length": bytes.len(),
        "sha256": sha256_hex(&bytes),
        "provenance": provenance,
        "git_identity": "excluded",
    })
}

fn corpus_executable_descriptor(path: &Path) -> serde_json::Value {
    let bytes = std::fs::read(path).expect("read corpus executable");
    serde_json::json!({
        "path": path,
        "length": bytes.len(),
        "sha256": sha256_hex(&bytes),
        "git_identity": "excluded",
    })
}

fn corpus_file_identity(path: &Path) -> ReleaseProgramFileIdentity {
    let bytes = std::fs::read(path).expect("read corpus bound file");
    ReleaseProgramFileIdentity {
        path: path
            .to_str()
            .expect("corpus temporary path is UTF-8")
            .to_owned(),
        bytes: u64::try_from(bytes.len()).expect("corpus file length fits u64"),
        sha256: sha256_hex(&bytes),
    }
}

fn replace_path(source: &Path, destination: &Path) {
    match std::fs::remove_file(destination) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => panic!("remove replacement destination: {error}"),
    }
    std::fs::rename(source, destination).expect("replace corpus path");
}

#[cfg(unix)]
fn symlink_file_for_corpus(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn symlink_file_for_corpus(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}

#[cfg(unix)]
fn symlink_directory_for_corpus(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn symlink_directory_for_corpus(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}

struct CorpusHarness {
    _directory: TempDirectory,
    root: PathBuf,
    directory: PathBuf,
    full_manifest_path: PathBuf,
    contract_path: PathBuf,
    receipt_path: PathBuf,
    data_path: PathBuf,
    full_manifest: serde_json::Value,
    characterization_manifest: serde_json::Value,
    legacy_manifest: serde_json::Value,
    legacy_readiness: serde_json::Value,
    contract: serde_json::Value,
    full_manifest_bytes: Vec<u8>,
    data_bytes: Vec<u8>,
    receipt_bytes: Vec<u8>,
    contract_bytes: Vec<u8>,
}

impl CorpusHarness {
    fn new() -> Self {
        let temporary = TempDirectory::new("shared-corpus");
        let directory = temporary.0.clone();
        let repository = PrivateRepository::discover().expect("discover fn64 repository");
        let root = repository.root().to_owned();

        let text_path = directory.join("synthetic-text.bin");
        let data_path = directory.join("synthetic-data.bin");
        let raw_text_path = directory.join("synthetic-text-raw-window.bin");
        let raw_data_path = directory.join("synthetic-data-raw-window.bin");
        let rom_path = directory.join("synthetic-rom.bin");
        let recompiled_path = directory.join("synthetic-recompiled.bin");
        let executable_path = directory.join(if cfg!(windows) {
            "synthetic-runner.exe"
        } else {
            "synthetic-runner"
        });
        std::fs::write(
            &text_path,
            (0..4096)
                .map(|index| ((index * 17 + 3) & 0xff) as u8)
                .collect::<Vec<_>>(),
        )
        .expect("write corpus text");
        std::fs::write(
            &data_path,
            (0..256)
                .map(|index| ((index * 29 + 5) & 0xff) as u8)
                .collect::<Vec<_>>(),
        )
        .expect("write corpus data");
        std::fs::write(
            &raw_text_path,
            (0..0x18d0)
                .map(|index| ((index * 37 + 7) & 0xff) as u8)
                .collect::<Vec<_>>(),
        )
        .expect("write corpus raw text");
        std::fs::write(
            &raw_data_path,
            (0..0x0fc0)
                .map(|index| ((index * 41 + 11) & 0xff) as u8)
                .collect::<Vec<_>>(),
        )
        .expect("write corpus raw data");
        std::fs::write(&rom_path, (0_u8..64).collect::<Vec<_>>())
            .expect("write corpus ROM fixture");
        std::fs::write(&recompiled_path, (0_u8..64).rev().collect::<Vec<_>>())
            .expect("write corpus recompiled fixture");
        std::fs::copy(
            std::env::current_exe().expect("resolve current test executable"),
            &executable_path,
        )
        .expect("copy native corpus executable");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&executable_path, std::fs::Permissions::from_mode(0o700))
                .expect("make corpus executable private and executable");
        }

        let executable_descriptor = corpus_executable_descriptor(&executable_path);
        let recompiled_identity = corpus_file_identity(&recompiled_path);
        let program_sha256 = "11".repeat(32);
        let execution_source = ExecutionDestinationSource::TypedBlockProgram {
            program_sha256: program_sha256.clone(),
            dispatch_artifact_sha256: recompiled_identity.sha256.clone(),
        };
        let mut receipt = ReleaseProgramBuildReceipt {
            schema: PROGRAM_BUILD_RECEIPT_SCHEMA.to_owned(),
            child_executable: corpus_file_identity(&executable_path),
            lane: ReleaseProgramBuildLane::TypedBlock {
                pack: recompiled_identity,
                expected_program_sha256: program_sha256,
            },
            expected_execution_source: execution_source.clone(),
            receipt_sha256: String::new(),
        };
        receipt.receipt_sha256 = receipt
            .recompute_receipt_sha256()
            .expect("compute corpus receipt digest");
        let receipt_path = directory.join("typed-block-program-receipt.json");
        let receipt_bytes =
            serialize_json_document(&receipt, "corpus receipt").expect("serialize receipt");
        std::fs::write(&receipt_path, &receipt_bytes).expect("write corpus receipt");

        let full_manifest = serde_json::json!({
            "schema": MANIFEST_SCHEMA,
            "purpose": "full_rom",
            "intent": {
                "wire_family": "full_rom_mixed",
                "report_scenario": "synthetic-private-admission-corpus",
                "recognition": "runtime_must_confirm_backend_known_pair",
                "extended_gbi_cases": [],
                "program_evidence_lane": "typed_block_program",
                "rom_class": "retail_cartridge",
                "characterization_suite": null
            },
            "release_matrix": {
                "platform": "macos_arm64",
                "controllers": ["standard_controller"],
                "save": "no_cartridge_save",
                "renderers": ["reference_lle_accuracy"],
                "repeat_bar": 10
            },
            "artifacts": {
                "microcode_text": corpus_descriptor(
                    &text_path,
                    "user_owned_rom_derived"
                ),
                "microcode_data": corpus_descriptor(
                    &data_path,
                    "user_owned_rom_derived"
                ),
                "microcode_text_raw_window": null,
                "microcode_data_raw_window": null,
                "rom": corpus_descriptor(
                    &rom_path,
                    "user_owned_retail_cartridge_dump"
                ),
                "recompiled": corpus_descriptor(
                    &recompiled_path,
                    "user_generated_from_owned_rom"
                )
            },
            "runner": {
                "executable": executable_descriptor,
                "working_directory": &directory,
                "argv": ["--synthetic"],
                "env": {"FN64_SYNTHETIC_FIXED": "1"},
                "release_gate_cycle": 42,
                "execution_source": execution_source,
                "program_build_receipt": corpus_executable_descriptor(&receipt_path)
            }
        });
        let full_manifest_path = directory.join("full-manifest.json");
        write_json_value(&full_manifest_path, &full_manifest);

        let readiness_path = directory.join("full-readiness.json");
        let admitted = admit_current_v7_manifest(&full_manifest_path, &readiness_path)
            .expect("admit corpus full-ROM baseline");
        std::fs::write(&readiness_path, &admitted.readiness_bytes)
            .expect("write corpus readiness baseline");
        let contract_bytes = admitted
            .contract_bytes
            .expect("full-ROM corpus admission emits a contract");
        let contract =
            serde_json::from_slice(&contract_bytes).expect("parse corpus contract baseline");
        let contract_path = directory.join("private-run-contract.json");
        std::fs::write(&contract_path, &contract_bytes).expect("write corpus contract");

        let mut extended = full_manifest.clone();
        extended["purpose"] = serde_json::json!("extended_gbi");
        extended["intent"]["wire_family"] = serde_json::json!("f3dex2_extended_gbi_v1");
        extended["intent"]["report_scenario"] =
            serde_json::json!("synthetic-private-admission-corpus-extended");
        extended["intent"]["extended_gbi_cases"] = serde_json::json!([
            "activation",
            "disabled-negative-control",
            "hook-control",
            "interpolation",
            "vertex-z",
            "widescreen"
        ]);
        extended["intent"]["program_evidence_lane"] = serde_json::json!("no_program_fixture");
        extended["intent"]["rom_class"] = serde_json::json!("not_applicable");
        extended["release_matrix"]["renderers"] =
            serde_json::json!(["rt64_lle_accuracy", "rt64_post_vi_capture"]);
        extended["artifacts"]["rom"] = serde_json::Value::Null;
        extended["artifacts"]["recompiled"] = serde_json::Value::Null;
        extended["runner"]["execution_source"] = serde_json::json!({"kind": "no_program"});
        extended["runner"]["program_build_receipt"] = serde_json::Value::Null;

        let mut characterization_manifest = extended.clone();
        characterization_manifest["purpose"] = serde_json::json!("f3dzex2_characterization");
        characterization_manifest["intent"]["wire_family"] = serde_json::json!("f3dzex2");
        characterization_manifest["intent"]["report_scenario"] =
            serde_json::json!("synthetic-f3dzex2-characterization-corpus");
        characterization_manifest["intent"]["extended_gbi_cases"] = serde_json::json!([]);
        characterization_manifest["intent"]["characterization_suite"] =
            serde_json::json!(F3DZEX2_CHARACTERIZATION_SUITE);
        characterization_manifest["artifacts"]["microcode_text"] = serde_json::Value::Null;
        characterization_manifest["artifacts"]["microcode_data"] = serde_json::Value::Null;
        characterization_manifest["artifacts"]["microcode_text_raw_window"] =
            corpus_descriptor(&raw_text_path, "user_owned_rom_derived");
        characterization_manifest["artifacts"]["microcode_data_raw_window"] =
            corpus_descriptor(&raw_data_path, "user_owned_rom_derived");

        let mut legacy_manifest = extended;
        legacy_manifest["schema"] = serde_json::json!(LEGACY_MANIFEST_SCHEMA);
        legacy_manifest["intent"]
            .as_object_mut()
            .expect("legacy intent object")
            .remove("characterization_suite");
        legacy_manifest["artifacts"]
            .as_object_mut()
            .expect("legacy artifacts object")
            .remove("microcode_text_raw_window");
        legacy_manifest["artifacts"]
            .as_object_mut()
            .expect("legacy artifacts object")
            .remove("microcode_data_raw_window");
        let stored = parse_manifest(
            &serde_json::to_vec(&legacy_manifest).expect("serialize legacy manifest"),
            "corpus legacy manifest",
        )
        .expect("parse legacy corpus manifest");
        let StoredManifest::V6(legacy) = stored else {
            panic!("corpus legacy manifest lost its v6 schema");
        };
        let validated =
            validate_manifest_v6(&repository, *legacy).expect("validate legacy corpus manifest");
        let legacy_readiness = derive_readiness(&validated)
            .and_then(|readiness| {
                validate_readiness(&readiness)?;
                serde_json::to_value(readiness)
                    .map_err(|source| error(format!("serialize legacy readiness: {source}")))
            })
            .expect("derive legacy corpus readiness");

        let full_manifest_bytes =
            std::fs::read(&full_manifest_path).expect("snapshot full manifest");
        let data_bytes = std::fs::read(&data_path).expect("snapshot corpus data");

        Self {
            _directory: temporary,
            root,
            directory,
            full_manifest_path,
            contract_path,
            receipt_path,
            data_path,
            full_manifest,
            characterization_manifest,
            legacy_manifest,
            legacy_readiness,
            contract,
            full_manifest_bytes,
            data_bytes,
            receipt_bytes,
            contract_bytes,
        }
    }

    fn disposable_paths(&self) -> [PathBuf; 8] {
        [
            self.directory.join("corpus-document.json"),
            self.directory.join("corpus-output.json"),
            self.directory.join("corpus-symlink-manifest.json"),
            self.directory.join("corpus-artifact-parent"),
            self.directory.join("corpus-working-directory"),
            self.directory.join("corpus-same-length-data.bin"),
            self.directory.join("corpus-contract-replacement.json"),
            self.directory.join("corpus-symlink-probe-link"),
        ]
    }

    fn reset(&self) {
        std::fs::write(&self.full_manifest_path, &self.full_manifest_bytes)
            .expect("restore corpus manifest");
        std::fs::write(&self.data_path, &self.data_bytes).expect("restore corpus data");
        std::fs::write(&self.receipt_path, &self.receipt_bytes).expect("restore corpus receipt");
        std::fs::write(&self.contract_path, &self.contract_bytes).expect("restore corpus contract");
        for path in self.disposable_paths() {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
                Err(source) if source.kind() == std::io::ErrorKind::IsADirectory => {
                    std::fs::remove_dir_all(&path).expect("remove disposable directory");
                }
                Err(source) => panic!("remove disposable corpus path: {source}"),
            }
        }
    }

    fn supports(&self, capability: CorpusCapability) -> bool {
        match capability {
            CorpusCapability::All => true,
            CorpusCapability::CaseInsensitive => {
                let canonical = self.root.join("README.md");
                let alias = self.root.join("readme.md");
                alias != canonical
                    && alias.exists()
                    && std::fs::canonicalize(alias).ok() == std::fs::canonicalize(canonical).ok()
            }
            CorpusCapability::Symlink => {
                let target = self.directory.join("corpus-symlink-probe-target");
                let link = self.directory.join("corpus-symlink-probe-link");
                std::fs::write(&target, b"synthetic").expect("write symlink probe target");
                let supported = symlink_file_for_corpus(&target, &link).is_ok();
                let _ = std::fs::remove_file(&link);
                std::fs::remove_file(&target).expect("remove symlink probe target");
                supported
            }
        }
    }

    fn validate_manifest_value(
        &self,
        value: &serde_json::Value,
    ) -> Result<ValidatedManifest, PrivateInputAdmissionError> {
        let repository = map_fs(PrivateRepository::discover(), "discover fn64 repository")?;
        match parse_manifest(
            &serde_json::to_vec(value)
                .map_err(|source| error(format!("serialize corpus manifest: {source}")))?,
            "corpus manifest",
        )? {
            StoredManifest::V7(manifest) => validate_manifest_v7(&repository, *manifest),
            StoredManifest::V6(manifest) => validate_manifest_v6(&repository, *manifest),
        }
    }

    fn validate_readiness_value(
        &self,
        value: &serde_json::Value,
    ) -> Result<(), PrivateInputAdmissionError> {
        let readiness: StoredReadiness = serde_json::from_value(value.clone())
            .map_err(|source| error(format!("parse readiness report: {source}")))?;
        validate_readiness(&readiness)
    }

    fn validate_contract_value(
        &self,
        value: &serde_json::Value,
    ) -> Result<(), PrivateInputAdmissionError> {
        let document = self.directory.join("corpus-document.json");
        write_json_value(&document, value);
        verify_retained_private_run_contract(&document).map(|_| ())
    }

    fn validate_semantics(
        &self,
        semantics: CorpusSemantics,
        manifest: Option<&ValidatedManifest>,
        contract: Option<&serde_json::Value>,
    ) -> Result<(), PrivateInputAdmissionError> {
        match semantics {
            CorpusSemantics::V7FullRom => {
                let manifest = manifest.ok_or_else(|| error("missing full-ROM semantics"))?;
                if manifest.schema != MANIFEST_SCHEMA
                    || manifest.purpose != Purpose::FullRom
                    || manifest.program_lane != ProgramEvidenceLane::TypedBlockProgram
                    || manifest.artifacts.keys().copied().collect::<BTreeSet<_>>()
                        != BTreeSet::from([
                            ArtifactRole::MicrocodeData,
                            ArtifactRole::MicrocodeText,
                            ArtifactRole::Recompiled,
                            ArtifactRole::Rom,
                        ])
                {
                    return Err(error("corpus v7 full-ROM accepted semantics drifted"));
                }
            }
            CorpusSemantics::V7Characterization => {
                let manifest =
                    manifest.ok_or_else(|| error("missing characterization semantics"))?;
                if manifest.schema != MANIFEST_SCHEMA
                    || manifest.purpose != Purpose::F3dzex2Characterization
                    || manifest.wire_family != WireFamily::F3dzex2
                    || manifest.artifacts.keys().copied().collect::<BTreeSet<_>>()
                        != BTreeSet::from([
                            ArtifactRole::MicrocodeDataRawWindow,
                            ArtifactRole::MicrocodeTextRawWindow,
                        ])
                {
                    return Err(error(
                        "corpus v7 characterization accepted semantics drifted",
                    ));
                }
                let readiness = derive_readiness(manifest)?;
                let StoredReadiness::V6(readiness) = readiness else {
                    return Err(error("characterization emitted retained readiness"));
                };
                if readiness.characterization_suite != F3DZEX2_CHARACTERIZATION_SUITE
                    || readiness.required_characterization_cases
                        != F3DZEX2_CHARACTERIZATION_CASES.map(str::to_owned)
                {
                    return Err(error("characterization denominator drifted"));
                }
            }
            CorpusSemantics::V6Retained => {
                let manifest = manifest.ok_or_else(|| error("missing v6 semantics"))?;
                if manifest.schema != LEGACY_MANIFEST_SCHEMA
                    || manifest.artifacts.keys().copied().collect::<BTreeSet<_>>()
                        != BTreeSet::from([
                            ArtifactRole::MicrocodeData,
                            ArtifactRole::MicrocodeText,
                        ])
                    || !matches!(derive_readiness(manifest)?, StoredReadiness::V5(_))
                {
                    return Err(error("corpus retained v6 accepted semantics drifted"));
                }
            }
            CorpusSemantics::V3Contract | CorpusSemantics::CapturedV3Contract => {
                let contract = contract.ok_or_else(|| error("missing contract semantics"))?;
                let contract: PrivateReleaseRunContract = serde_json::from_value(contract.clone())
                    .map_err(|source| error(format!("parse accepted corpus contract: {source}")))?;
                contract.verify_integrity().map_err(|source| {
                    error(format!("verify accepted corpus contract: {source}"))
                })?;
            }
        }
        Ok(())
    }

    fn execute_recipe(&self, recipe: &CorpusRecipe) -> Result<(), PrivateInputAdmissionError> {
        use CorpusBaseline as B;
        use CorpusOperationKind as O;

        match recipe.operation.kind {
            O::Validate => match recipe.baseline {
                B::ManifestV7FullRom => {
                    let manifest = self.validate_manifest_value(&self.full_manifest)?;
                    self.validate_semantics(
                        recipe.semantics.expect("accepted recipe has semantics"),
                        Some(&manifest),
                        None,
                    )
                }
                B::ManifestV7Characterization => {
                    let manifest = self.validate_manifest_value(&self.characterization_manifest)?;
                    self.validate_semantics(
                        recipe.semantics.expect("accepted recipe has semantics"),
                        Some(&manifest),
                        None,
                    )
                }
                B::ManifestV6Retained => {
                    let manifest = self.validate_manifest_value(&self.legacy_manifest)?;
                    self.validate_semantics(
                        recipe.semantics.expect("accepted recipe has semantics"),
                        Some(&manifest),
                        None,
                    )
                }
                B::ContractV3FromV7 => {
                    self.validate_contract_value(&self.contract)?;
                    self.validate_semantics(
                        recipe.semantics.expect("accepted recipe has semantics"),
                        None,
                        Some(&self.contract),
                    )
                }
                _ => unreachable!("strict corpus validation rejected invalid baseline"),
            },
            O::DuplicateRootField => serde_json::from_str::<StrictUniqueObject>(
                r#"{"schema":"first","schema":"second"}"#,
            )
            .map(|_| ())
            .map_err(|source| error(format!("parse corpus manifest: {source}"))),
            O::DuplicateNestedEnvField => {
                serde_json::from_str::<StrictEnvironment>(r#"{"A":"1","A":"2"}"#)
                    .map(|_| ())
                    .map_err(|source| error(format!("parse corpus runner.env: {source}")))
            }
            O::SymlinkManifest => {
                let link = self.directory.join("corpus-symlink-manifest.json");
                symlink_file_for_corpus(&self.full_manifest_path, &link)
                    .expect("create corpus manifest symlink after capability probe");
                admit_current_v7_manifest(&link, &self.directory.join("corpus-output.json"))
                    .map(|_| ())
            }
            O::SymlinkArtifactParent => {
                let link = self.directory.join("corpus-artifact-parent");
                symlink_directory_for_corpus(&self.directory, &link)
                    .expect("create corpus artifact-parent symlink after capability probe");
                let mut value = self.full_manifest.clone();
                value["artifacts"]["microcode_data"]["path"] = serde_json::json!(
                    link.join(self.data_path.file_name().expect("corpus data has a name"))
                );
                self.validate_manifest_value(&value).map(|_| ())
            }
            O::SymlinkWorkingDirectory => {
                let link = self.directory.join("corpus-working-directory");
                symlink_directory_for_corpus(&self.directory, &link)
                    .expect("create corpus working-directory symlink after capability probe");
                let mut value = self.full_manifest.clone();
                value["runner"]["working_directory"] = serde_json::json!(link);
                self.validate_manifest_value(&value).map(|_| ())
            }
            O::SameLengthDescriptorSwap => {
                let alternate = self.directory.join("corpus-same-length-data.bin");
                let altered = self
                    .data_bytes
                    .iter()
                    .map(|byte| byte ^ 0xff)
                    .collect::<Vec<_>>();
                assert_eq!(altered.len(), self.data_bytes.len());
                assert_ne!(sha256_hex(&altered), sha256_hex(&self.data_bytes));
                std::fs::write(&alternate, altered).expect("write alternate corpus data");
                let mut value = self.full_manifest.clone();
                value["artifacts"]["microcode_data"]["path"] = serde_json::json!(alternate);
                self.validate_manifest_value(&value).map(|_| ())
            }
            O::SameLengthPathSwap => {
                let alternate = self.directory.join("corpus-same-length-data.bin");
                let altered = self
                    .data_bytes
                    .iter()
                    .map(|byte| byte ^ 0xff)
                    .collect::<Vec<_>>();
                assert_eq!(altered.len(), self.data_bytes.len());
                assert_ne!(sha256_hex(&altered), sha256_hex(&self.data_bytes));
                std::fs::write(&alternate, altered).expect("write alternate corpus data");
                replace_path(&alternate, &self.data_path);
                self.validate_manifest_value(&self.full_manifest)
                    .map(|_| ())
            }
            O::CapturedContractSourceReplacement => {
                let repository = PrivateRepository::discover()
                    .expect("discover repository for stable contract capture");
                let captured =
                    read_private_file(&repository, &self.contract_path, "corpus captured contract")
                        .expect("capture corpus contract through stable handle");
                let replacement = self.directory.join("corpus-contract-replacement.json");
                assert!(self.contract_bytes.len() >= 3);
                let mut replacement_bytes = vec![b' '; self.contract_bytes.len()];
                replacement_bytes[0] = b'{';
                replacement_bytes[1] = b'}';
                let last = replacement_bytes.len() - 1;
                replacement_bytes[last] = b'\n';
                std::fs::write(&replacement, replacement_bytes)
                    .expect("write same-length contract replacement");
                replace_path(&replacement, &self.contract_path);
                verify_retained_contract_read(&repository, captured).and_then(|_| {
                    self.validate_semantics(
                        recipe.semantics.expect("captured recipe has semantics"),
                        None,
                        Some(&self.contract),
                    )
                })
            }
            O::RelativeArtifactPath | O::ParentArtifactPath => {
                let mut value = self.full_manifest.clone();
                value["artifacts"]["microcode_data"]["path"] =
                    if recipe.operation.kind == O::RelativeArtifactPath {
                        serde_json::json!(self
                            .data_path
                            .file_name()
                            .expect("corpus data has a name")
                            .to_string_lossy())
                    } else {
                        serde_json::json!(self
                            .directory
                            .join("missing")
                            .join("..")
                            .join(self.data_path.file_name().expect("corpus data has a name")))
                    };
                self.validate_manifest_value(&value).map(|_| ())
            }
            O::TrackedArtifactPath | O::CaseAliasTrackedPath => {
                let tracked = self
                    .root
                    .join(if recipe.operation.kind == O::CaseAliasTrackedPath {
                        "readme.md"
                    } else {
                        "README.md"
                    });
                if recipe.operation.kind == O::CaseAliasTrackedPath {
                    assert_eq!(
                        std::fs::canonicalize(&tracked).expect("resolve README case alias"),
                        std::fs::canonicalize(self.root.join("README.md"))
                            .expect("resolve canonical README")
                    );
                }
                let mut value = self.full_manifest.clone();
                value["artifacts"]["microcode_data"] =
                    corpus_descriptor(&tracked, "user_owned_rom_derived");
                self.validate_manifest_value(&value).map(|_| ())
            }
            O::ReservedRomEnv
            | O::FutureReleaseEnv
            | O::LdPreloadEnv
            | O::D3d12sdkPathEnv
            | O::LowercaseEnv => {
                let name = match recipe.operation.kind {
                    O::ReservedRomEnv => "ROM",
                    O::FutureReleaseEnv => "FN64_RELEASE_FUTURE",
                    O::LdPreloadEnv => "LD_PRELOAD",
                    O::D3d12sdkPathEnv => "D3D12SDK_PATH",
                    O::LowercaseEnv => "lowercase",
                    _ => unreachable!(),
                };
                let mut value = self.full_manifest.clone();
                value["runner"]["env"][name] = serde_json::json!("synthetic");
                self.validate_manifest_value(&value).map(|_| ())
            }
            O::ArtifactTamper => {
                let mut tampered = self.data_bytes.clone();
                tampered.extend_from_slice(b"synthetic");
                assert_ne!(tampered.len(), self.data_bytes.len());
                std::fs::write(&self.data_path, tampered).expect("tamper corpus artifact");
                self.validate_manifest_value(&self.full_manifest)
                    .map(|_| ())
            }
            O::ReceiptInnerTamper => {
                let mut receipt: ReleaseProgramBuildReceipt =
                    serde_json::from_slice(&self.receipt_bytes)
                        .expect("parse corpus receipt for inner tamper");
                let ReleaseProgramBuildLane::TypedBlock {
                    expected_program_sha256,
                    ..
                } = &mut receipt.lane
                else {
                    panic!("corpus receipt lane drifted");
                };
                *expected_program_sha256 = "22".repeat(32);
                receipt.receipt_sha256 = receipt
                    .recompute_receipt_sha256()
                    .expect("resign inner-tampered receipt");
                let bytes = serialize_json_document(&receipt, "tampered corpus receipt")?;
                std::fs::write(&self.receipt_path, bytes).expect("write inner-tampered receipt");
                let mut value = self.full_manifest.clone();
                value["runner"]["program_build_receipt"] =
                    corpus_executable_descriptor(&self.receipt_path);
                self.validate_manifest_value(&value).map(|_| ())
            }
            O::ReceiptOuterTamper => {
                let mut tampered = self.receipt_bytes.clone();
                tampered.push(b' ');
                std::fs::write(&self.receipt_path, tampered).expect("write outer-tampered receipt");
                self.validate_manifest_value(&self.full_manifest)
                    .map(|_| ())
            }
            O::ContractDigestTamper => {
                let mut value = self.contract.clone();
                value["contract_sha256"] = serde_json::json!("00".repeat(32));
                self.validate_contract_value(&value)
            }
            O::ContractDescriptorTamper => {
                let mut contract: PrivateReleaseRunContract =
                    serde_json::from_value(self.contract.clone())
                        .expect("parse corpus contract for descriptor tamper");
                contract.admission_manifest.sha256 = "00".repeat(32);
                contract.contract_sha256 = contract
                    .recompute_contract_sha256()
                    .expect("resign descriptor-tampered contract");
                let value =
                    serde_json::to_value(contract).expect("serialize descriptor-tampered contract");
                self.validate_contract_value(&value)
            }
            O::V6NewAdmission => {
                let document = self.directory.join("corpus-document.json");
                let output = self.directory.join("corpus-output.json");
                write_json_value(&document, &self.legacy_manifest);
                let result = admit_current_v7_manifest(&document, &output).map(|_| ());
                assert!(
                    !output.exists(),
                    "rejected retained-v6 admission published an output"
                );
                result
            }
            O::V6WithV7Field => {
                let mut value = self.legacy_manifest.clone();
                value["intent"]["characterization_suite"] = serde_json::Value::Null;
                self.validate_manifest_value(&value).map(|_| ())
            }
            O::V6F3dzex2RawRole => {
                let mut value = self.legacy_manifest.clone();
                value["intent"]["wire_family"] = serde_json::json!("f3dzex2");
                value["artifacts"]["microcode_text_raw_window"] = self.characterization_manifest
                    ["artifacts"]["microcode_text_raw_window"]
                    .clone();
                self.validate_manifest_value(&value).map(|_| ())
            }
            O::V5WithCurrentField => {
                let mut value = self.legacy_readiness.clone();
                value["characterization_suite"] = serde_json::json!("not_requested");
                self.validate_readiness_value(&value)
            }
        }
    }
}

fn assert_rust_rejection_cause(operation: CorpusOperationKind, message: &str) {
    use CorpusOperationKind as O;
    let require = |fragments: &[&str]| {
        assert!(
            fragments.iter().all(|fragment| message.contains(fragment)),
            "{operation:?} rejected for an unrelated reason: {message}"
        );
    };
    match operation {
        O::DuplicateRootField => require(&["duplicate field", "schema"]),
        O::DuplicateNestedEnvField => require(&["duplicate field", "A"]),
        O::SymlinkManifest => require(&["manifest", "symlink"]),
        O::SymlinkArtifactParent => require(&["artifacts.microcode_data", "symlink"]),
        O::SymlinkWorkingDirectory => require(&["runner.working_directory", "symlink"]),
        O::SameLengthDescriptorSwap | O::SameLengthPathSwap => {
            require(&["artifacts.microcode_data", "identity drift"])
        }
        O::RelativeArtifactPath => require(&["artifacts.microcode_data", "absolute"]),
        O::ParentArtifactPath => require(&["artifacts.microcode_data", "'..'"]),
        O::TrackedArtifactPath | O::CaseAliasTrackedPath => {
            assert!(
                message.contains("artifacts.microcode_data")
                    && (message.contains("tracked by git")
                        || message.contains("inside the repository")),
                "{operation:?} rejected for an unrelated reason: {message}"
            );
        }
        O::ReservedRomEnv => require(&["ROM", "reserved"]),
        O::FutureReleaseEnv => require(&["FN64_RELEASE_FUTURE", "reserved"]),
        O::LdPreloadEnv => require(&["LD_PRELOAD", "inject"]),
        O::D3d12sdkPathEnv => require(&["D3D12SDK_PATH", "inject"]),
        O::LowercaseEnv => require(&["lowercase", "invalid"]),
        O::ArtifactTamper => require(&["artifacts.microcode_data", "identity drift"]),
        O::ReceiptInnerTamper => require(&["execution source", "mismatch"]),
        O::ReceiptOuterTamper => require(&["runner.program_build_receipt", "identity drift"]),
        O::ContractDigestTamper => require(&["contract", "digest", "mismatch"]),
        O::ContractDescriptorTamper => require(&["contract.admission_manifest", "identity drift"]),
        O::V6NewAdmission => require(&["new admission", "schema"]),
        O::V6WithV7Field | O::V5WithCurrentField => require(&["characterization_suite"]),
        O::V6F3dzex2RawRole => {
            assert!(
                message.contains("f3dzex2") || message.contains("microcode_text_raw_window"),
                "v6 mixed-new-field recipe rejected for an unrelated reason: {message}"
            );
        }
        O::Validate | O::CapturedContractSourceReplacement => {
            panic!("accepted corpus operation reached rejection classifier")
        }
    }
}

fn rust_private_admission_corpus_snapshot() -> CorpusSnapshotDocument {
    let corpus = load_private_admission_corpus();
    let harness = CorpusHarness::new();
    let required_all = corpus
        .recipes
        .iter()
        .filter(|recipe| recipe.capability == CorpusCapability::All)
        .map(|recipe| recipe.id.clone())
        .collect::<BTreeSet<_>>();
    let mut executed_all = BTreeSet::new();
    let mut results = Vec::with_capacity(corpus.recipes.len());

    for recipe in &corpus.recipes {
        harness.reset();
        if !harness.supports(recipe.capability) {
            assert_ne!(
                recipe.capability,
                CorpusCapability::All,
                "capability=all recipe {} was skipped",
                recipe.id
            );
            results.push(CorpusSnapshotResult {
                id: recipe.id.clone(),
                verdict: "skip".to_owned(),
            });
            continue;
        }
        let result = harness.execute_recipe(recipe);
        let accepted = result.is_ok();
        assert_eq!(
            accepted,
            recipe.expect == CorpusExpectation::Accept,
            "recipe {} verdict drifted: {:?}",
            recipe.id,
            result.as_ref().err()
        );
        if let Err(source) = result {
            assert_rust_rejection_cause(recipe.operation.kind, &source.to_string());
        }
        if recipe.capability == CorpusCapability::All {
            executed_all.insert(recipe.id.clone());
        }
        results.push(CorpusSnapshotResult {
            id: recipe.id.clone(),
            verdict: if accepted { "accept" } else { "reject" }.to_owned(),
        });
    }
    assert_eq!(executed_all, required_all);
    harness.reset();
    CorpusSnapshotDocument {
        schema: "fn64.private-admission-corpus-snapshot.v1".to_owned(),
        results,
    }
}

#[test]
fn shared_content_free_corpus_matches_rust_policy() {
    let snapshot = rust_private_admission_corpus_snapshot();
    assert_eq!(snapshot.results.len(), 30);
    assert_eq!(
        snapshot
            .results
            .iter()
            .filter(|result| result.verdict == "accept")
            .count(),
        5
    );
    assert_eq!(
        snapshot
            .results
            .iter()
            .filter(|result| matches!(result.verdict.as_str(), "reject" | "skip"))
            .count(),
        25,
        "host-gated rejection recipes must reject or explicitly skip"
    );
    assert!(
        snapshot
            .results
            .iter()
            .filter(|result| result.verdict == "reject")
            .count()
            >= 21,
        "all capability=all rejection recipes must execute"
    );
}

#[test]
fn corpus_validator_rejects_private_identity_material() {
    for private in [
        serde_json::json!({"path": "/private/input"}),
        serde_json::json!({"nested": {"sha256": "not-even-a-digest"}}),
        serde_json::json!({"value": "a".repeat(64)}),
    ] {
        assert!(
            reject_corpus_identity_material(&private, "synthetic corpus").is_err(),
            "accepted private identity material {private}"
        );
    }
}

#[cfg(unix)]
#[test]
fn python_and_rust_whole_corpus_snapshots_match() {
    let rust = rust_private_admission_corpus_snapshot();
    let repository = PrivateRepository::discover().expect("discover fn64 repository");
    let output = Command::new("python3")
        .arg(repository.root().join("tools/private_input_admission.py"))
        .arg("--corpus-snapshot")
        .current_dir(repository.root())
        .output()
        .expect("run Python private-admission corpus snapshot");
    assert!(
        output.status.success(),
        "Python private-admission corpus failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let python: CorpusSnapshotDocument = serde_json::from_slice(&output.stdout)
        .expect("parse strict Python content-free corpus snapshot");
    assert_eq!(python, rust);
}
