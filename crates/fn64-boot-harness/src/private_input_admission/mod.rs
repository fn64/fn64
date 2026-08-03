//! In-process authority policy for private release inputs.
//!
//! This module is intentionally split from filesystem mechanics. Every path
//! is opened and measured through `private_fs`; policy never performs a
//! check-then-`fs::read` sequence of its own. The accepted documents and
//! canonical digest wires are the Rust transcription of the repository-owned
//! `tools/private_input_admission.py` v7/v6 admission contract.


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

mod admission;
mod readiness;

pub use admission::*;
// readiness's items are all module-internal; a pub glob would re-export
// nothing public and rustc warns. Siblings reach them via the parent scope.
use readiness::*;


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
        include!("../private_input_admission_corpus_tests.rs");
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
