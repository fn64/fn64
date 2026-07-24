//! Fresh-process proof that the private-series runner drives the live
//! runtime/device/reference-render release path, not a report-file fixture.

#[cfg(feature = "synthetic-native-archive-evidence")]
extern crate fn64_render_rt64 as _fn64_render_rt64;

#[allow(dead_code)]
#[path = "../examples/synthetic_fixed_cycle_release.rs"]
mod synthetic_fixed_cycle_release;

use fn64_boot_harness::{
    parse_unsupported_journal, ClosurePathStatus, ExecutionDestinationSource, ReleaseGateReport,
    RspRdpObservationKindEvidence, PRIVATE_RELEASE_SERIES_COUNT,
    REPOSITORY_SYNTHETIC_RELEASE_CYCLE,
};
use fn64_boot_harness::{
    run_private_release_series, verify_private_release_series, PrivateArtifactIdentity,
    PrivateChildCommand, PrivateEnvironmentEntry, PrivateFileIdentity, PrivateReleaseRunContract,
    ReleaseRomClass, PRIVATE_RELEASE_RUN_CONTRACT_SCHEMA, REPOSITORY_SYNTHETIC_RELEASE_INPUT_BYTES,
    REPOSITORY_SYNTHETIC_RELEASE_MANIFEST_BYTES, REPOSITORY_SYNTHETIC_RELEASE_READINESS_BYTES,
};
#[cfg(feature = "synthetic-native-archive-evidence")]
use fn64_boot_harness::{
    verify_release_matrix, verify_repository_synthetic_native_private_release_run_contract,
    ArtifactDigest, CertificationProfileIdentity, CertificationRequirementClass, ClosurePath,
    ExecutionDestinationEvidence, ReleaseMatrixManifest, ReleaseMatrixScenario,
    ReleaseMatrixVerification, RspRdpEvidence, UnsupportedInstrumentationEvidence,
    RELEASE_MATRIX_SCHEMA, REPOSITORY_SYNTHETIC_NATIVE_RELEASE_SCENARIO,
};
#[cfg(not(feature = "synthetic-native-archive-evidence"))]
use fn64_boot_harness::{
    verify_repository_synthetic_private_release_run_contract, REPOSITORY_SYNTHETIC_RELEASE_SCENARIO,
};
#[cfg(feature = "synthetic-native-archive-evidence")]
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const CHILD_ENV: &str = "FN64_TEST_PRIVATE_RELEASE_CHILD";
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[cfg(feature = "synthetic-native-archive-evidence")]
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SyntheticSemanticFingerprint {
    schema: String,
    scenario: String,
    input_sha256: String,
    guest_cycle: u64,
    artifacts: Vec<ArtifactDigest>,
    artifact_root_sha256: String,
    unsupported_instrumentation: UnsupportedInstrumentationEvidence,
    closure: Vec<ClosurePath>,
    execution_destinations: ExecutionDestinationEvidence,
    rsp_rdp: RspRdpEvidence,
    report_sha256: String,
}

#[cfg(feature = "synthetic-native-archive-evidence")]
impl SyntheticSemanticFingerprint {
    fn from_report(report: &ReleaseGateReport) -> Self {
        Self {
            schema: report.schema.clone(),
            scenario: report.scenario.clone(),
            input_sha256: report.input_sha256.clone(),
            guest_cycle: report.digest.guest_cycle,
            artifacts: report.digest.artifacts.clone(),
            artifact_root_sha256: report.digest.root_sha256.clone(),
            unsupported_instrumentation: report.unsupported_instrumentation.clone(),
            closure: report.closure.clone(),
            execution_destinations: report.execution_destinations.clone(),
            rsp_rdp: report.rsp_rdp.clone(),
            report_sha256: report.report_sha256.clone(),
        }
    }
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let base = if Path::new("/private/tmp").is_dir() {
            PathBuf::from("/private/tmp")
        } else {
            std::env::temp_dir()
        };
        loop {
            let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = base.join(format!(
                "fn64-live-private-release-{}-{counter}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(source) => panic!("create test directory {}: {source}", path.display()),
            }
        }
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn file_identity(path: &Path) -> PrivateFileIdentity {
    let mut file = File::open(path).unwrap();
    let mut digest = Sha256::new();
    let mut bytes = 0u64;
    let mut buffer = [0u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer).unwrap();
        if read == 0 {
            break;
        }
        bytes += read as u64;
        digest.update(&buffer[..read]);
    }
    PrivateFileIdentity {
        path: path.to_str().unwrap().to_owned(),
        bytes,
        sha256: format!("{:x}", digest.finalize()),
    }
}

fn synthetic_input_identity(path: &Path) -> PrivateArtifactIdentity {
    let identity = file_identity(path);
    PrivateArtifactIdentity {
        role: "synthetic_input".to_owned(),
        path: identity.path,
        bytes: identity.bytes,
        sha256: identity.sha256,
        provenance: "repository_defined_synthetic".to_owned(),
    }
}

#[cfg(feature = "synthetic-native-archive-evidence")]
fn embedded_artifact_identity(path: &Path, bytes: &[u8], role: &str) -> PrivateArtifactIdentity {
    PrivateArtifactIdentity {
        role: role.to_owned(),
        path: path.to_str().unwrap().to_owned(),
        bytes: bytes.len().try_into().unwrap(),
        sha256: format!("{:x}", Sha256::digest(bytes)),
        provenance: "repository_defined_synthetic".to_owned(),
    }
}

#[test]
fn live_synthetic_child() {
    if std::env::var_os(CHILD_ENV).is_none() {
        return;
    }
    synthetic_fixed_cycle_release::run_from_release_environment().unwrap();
}

#[cfg(not(feature = "synthetic-native-archive-evidence"))]
#[test]
fn ten_fresh_processes_certify_live_runtime_device_and_render_path() {
    let directory = TestDirectory::new();
    let manifest = directory.0.join("synthetic-manifest.json");
    let readiness = directory.0.join("synthetic-readiness.json");
    let input = directory.0.join("synthetic-input.bin");
    fs::write(&manifest, REPOSITORY_SYNTHETIC_RELEASE_MANIFEST_BYTES).unwrap();
    fs::write(&readiness, REPOSITORY_SYNTHETIC_RELEASE_READINESS_BYTES).unwrap();
    fs::write(&input, REPOSITORY_SYNTHETIC_RELEASE_INPUT_BYTES).unwrap();

    let executable = std::env::current_exe().unwrap();
    let mut contract = PrivateReleaseRunContract {
        schema: PRIVATE_RELEASE_RUN_CONTRACT_SCHEMA.to_owned(),
        admission_manifest: file_identity(&manifest),
        readiness_report: file_identity(&readiness),
        program_build_receipt: None,
        purpose: "synthetic_mechanism".to_owned(),
        rom_class: ReleaseRomClass::Unclassified,
        report_scenario: REPOSITORY_SYNTHETIC_RELEASE_SCENARIO.to_owned(),
        guest_cycle: REPOSITORY_SYNTHETIC_RELEASE_CYCLE,
        repeat_count: PRIVATE_RELEASE_SERIES_COUNT,
        input: synthetic_input_identity(&input),
        admitted_artifacts: Vec::new(),
        expected_execution_source: ExecutionDestinationSource::NoProgram,
        child: PrivateChildCommand {
            executable: file_identity(&executable),
            working_directory: directory.0.to_str().unwrap().to_owned(),
            argv: vec![
                "--exact".to_owned(),
                "live_synthetic_child".to_owned(),
                "--nocapture".to_owned(),
            ],
            environment: vec![PrivateEnvironmentEntry {
                name: CHILD_ENV.to_owned(),
                value: "1".to_owned(),
            }],
        },
        contract_sha256: String::new(),
    };
    contract.contract_sha256 = contract.recompute_contract_sha256().unwrap();
    let contract = verify_repository_synthetic_private_release_run_contract(contract).unwrap();

    let output = directory.0.join("series");
    let receipt = run_private_release_series(&contract, &output).unwrap_or_else(|error| {
        let stdout = fs::read_to_string(output.join("run-01.stdout.log")).unwrap_or_default();
        let stderr = fs::read_to_string(output.join("run-01.stderr.log")).unwrap_or_default();
        panic!("{error}\nchild stdout:\n{stdout}\nchild stderr:\n{stderr}");
    });
    verify_private_release_series(&contract, &output, &receipt).unwrap();

    assert_eq!(receipt.count, PRIVATE_RELEASE_SERIES_COUNT);
    assert_eq!(
        receipt.report_scenario,
        REPOSITORY_SYNTHETIC_RELEASE_SCENARIO
    );
    assert_eq!(receipt.guest_cycle, REPOSITORY_SYNTHETIC_RELEASE_CYCLE);
    assert_eq!(
        receipt
            .runs
            .iter()
            .map(|run| run.run_event_sha256.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        PRIVATE_RELEASE_SERIES_COUNT
    );
    for ordinal in 1..=PRIVATE_RELEASE_SERIES_COUNT {
        assert!(output.join(format!("report-{ordinal:02}.json")).is_file());
        assert!(output
            .join(format!("report-{ordinal:02}.unsupported.jsonl"))
            .is_file());
    }
    assert!(output.join("receipt.json").is_file());

    load_xbus_evidence(&output);
}

fn load_xbus_evidence(
    output: &Path,
) -> Vec<(
    ReleaseGateReport,
    fn64_boot_harness::ParsedUnsupportedJournal,
)> {
    (1..=PRIVATE_RELEASE_SERIES_COUNT)
        .map(|ordinal| {
            let report: ReleaseGateReport = serde_json::from_slice(
                &fs::read(output.join(format!("report-{ordinal:02}.json"))).unwrap(),
            )
            .unwrap();
            assert!(report.closure.iter().all(|path| {
                path.status == ClosurePathStatus::ExercisedZeroUnsupported
                    && path.unsupported.is_empty()
            }));
            let audio_execution = report
                .closure
                .iter()
                .find(|path| path.name == "rsp.audio-task")
                .expect("live closure contains the audio execution path");
            assert_eq!(
                audio_execution.observations, 1,
                "one admitted-and-started synthetic audio task must produce exactly one execution observation"
            );
            assert!(report.rsp_rdp.ordered.iter().any(|event| matches!(
                &event.observation,
                RspRdpObservationKindEvidence::XbusDpcCommitted {
                    start: 0,
                    end: 40,
                    ..
                }
            )));
            let journal = parse_unsupported_journal(
                &fs::read(output.join(format!("report-{ordinal:02}.unsupported.jsonl"))).unwrap(),
            )
            .unwrap();
            (report, journal)
        })
        .collect()
}

#[cfg(feature = "synthetic-native-archive-evidence")]
#[test]
fn ten_fresh_native_processes_satisfy_the_xbus_dpc_matrix_requirement() {
    let directory = TestDirectory::new();
    let manifest = directory.0.join("synthetic-manifest.json");
    let readiness = directory.0.join("synthetic-readiness.json");
    let input = directory.0.join("synthetic-input.bin");
    fs::write(&manifest, REPOSITORY_SYNTHETIC_RELEASE_MANIFEST_BYTES).unwrap();
    fs::write(&readiness, REPOSITORY_SYNTHETIC_RELEASE_READINESS_BYTES).unwrap();
    fs::write(&input, REPOSITORY_SYNTHETIC_RELEASE_INPUT_BYTES).unwrap();

    let executable = std::env::current_exe().unwrap();
    let archives = [
        embedded_artifact_identity(
            Path::new(env!("FN64_SYNTHETIC_GENERATED_ARCHIVE")),
            include_bytes!(env!("FN64_SYNTHETIC_GENERATED_ARCHIVE")),
            "synthetic_generated_archive",
        ),
        embedded_artifact_identity(
            Path::new(env!("FN64_SYNTHETIC_BRIDGE_ARCHIVE")),
            include_bytes!(env!("FN64_SYNTHETIC_BRIDGE_ARCHIVE")),
            "synthetic_section_bridge_archive",
        ),
    ];
    let mut contract = PrivateReleaseRunContract {
        schema: PRIVATE_RELEASE_RUN_CONTRACT_SCHEMA.to_owned(),
        admission_manifest: file_identity(&manifest),
        readiness_report: file_identity(&readiness),
        program_build_receipt: None,
        purpose: "synthetic_mechanism".to_owned(),
        rom_class: ReleaseRomClass::Unclassified,
        report_scenario: REPOSITORY_SYNTHETIC_NATIVE_RELEASE_SCENARIO.to_owned(),
        guest_cycle: REPOSITORY_SYNTHETIC_RELEASE_CYCLE,
        repeat_count: PRIVATE_RELEASE_SERIES_COUNT,
        input: synthetic_input_identity(&input),
        admitted_artifacts: archives.clone().into(),
        expected_execution_source: ExecutionDestinationSource::NativeArchive {
            artifact_sha256: env!("FN64_SYNTHETIC_NATIVE_PROGRAM_SHA256").to_owned(),
        },
        child: PrivateChildCommand {
            executable: file_identity(&executable),
            working_directory: directory.0.to_str().unwrap().to_owned(),
            argv: vec![
                "--exact".to_owned(),
                "live_synthetic_child".to_owned(),
                "--nocapture".to_owned(),
            ],
            environment: vec![PrivateEnvironmentEntry {
                name: CHILD_ENV.to_owned(),
                value: "1".to_owned(),
            }],
        },
        contract_sha256: String::new(),
    };
    contract.contract_sha256 = contract.recompute_contract_sha256().unwrap();
    let expected_archives = archives;
    let expected_child = contract.child.clone();
    let contract = verify_repository_synthetic_native_private_release_run_contract(
        contract,
        expected_archives,
        expected_child,
    )
    .unwrap();
    let output = directory.0.join("series");
    let receipt = run_private_release_series(&contract, &output).unwrap_or_else(|error| {
        let stdout = fs::read_to_string(output.join("run-01.stdout.log")).unwrap_or_default();
        let stderr = fs::read_to_string(output.join("run-01.stderr.log")).unwrap_or_default();
        panic!("{error}\nchild stdout:\n{stdout}\nchild stderr:\n{stderr}");
    });
    verify_private_release_series(&contract, &output, &receipt).unwrap();
    assert_eq!(receipt.count, PRIVATE_RELEASE_SERIES_COUNT);
    assert_eq!(
        receipt.report_scenario,
        REPOSITORY_SYNTHETIC_NATIVE_RELEASE_SCENARIO
    );
    assert_eq!(
        receipt
            .runs
            .iter()
            .map(|run| run.run_event_sha256.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        PRIVATE_RELEASE_SERIES_COUNT
    );

    let evidence = load_xbus_evidence(&output);
    let semantic_report = &evidence[0].0;
    let expected_fingerprint: SyntheticSemanticFingerprint = serde_json::from_str(include_str!(
        "fixtures/synthetic_native_v28_fingerprint.json"
    ))
    .unwrap();
    for (report, _) in &evidence {
        assert_eq!(
            SyntheticSemanticFingerprint::from_report(report),
            expected_fingerprint
        );
    }
    let mut scenario = ReleaseMatrixScenario {
        id: "public-synthetic-xbus-dpc".to_owned(),
        report_scenario: semantic_report.scenario.clone(),
        input_sha256: semantic_report.input_sha256.clone(),
        report_sha256: semantic_report.report_sha256.clone(),
        declaration_sha256: String::new(),
    };
    scenario.declaration_sha256 = scenario.recompute_declaration_sha256();
    let manifest = ReleaseMatrixManifest {
        schema: RELEASE_MATRIX_SCHEMA.to_owned(),
        profile: CertificationProfileIdentity::full_parity_v1(),
        scenarios: vec![scenario],
    };
    let assessment = match verify_release_matrix(&manifest, &evidence).unwrap() {
        ReleaseMatrixVerification::Incomplete(assessment) => assessment,
        ReleaseMatrixVerification::Complete(_) => {
            panic!("one public synthetic scenario cannot complete FullParityV1")
        }
    };
    let expected = [
        (
            CertificationRequirementClass::ProgramRendererLane,
            "native_archive/reference_lle_accuracy",
        ),
        (CertificationRequirementClass::Save, "no_cartridge_save"),
        (
            CertificationRequirementClass::Controller,
            "standard_controller",
        ),
        (CertificationRequirementClass::RspRdpMechanism, "dram-dpc"),
        (CertificationRequirementClass::RspRdpMechanism, "xbus-dpc"),
    ];
    assert_eq!(assessment.satisfied.len(), expected.len());
    for (class, id) in expected {
        assert!(assessment.satisfied.iter().any(|assignment| {
            assignment.requirement.class() == class && assignment.requirement.id() == id
        }));
    }
    assert!(!assessment.missing.iter().any(|requirement| {
        requirement.class() == CertificationRequirementClass::RspRdpMechanism
            && requirement.id() == "xbus-dpc"
    }));
}
