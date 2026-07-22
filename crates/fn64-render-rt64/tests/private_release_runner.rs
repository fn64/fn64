//! Fresh-process proof that the private-series runner drives the live
//! runtime/device/reference-render release path, not a report-file fixture.

#[cfg(feature = "synthetic-native-archive-evidence")]
extern crate fn64_render_rt64 as _fn64_render_rt64;

#[allow(dead_code)]
#[path = "../examples/synthetic_fixed_cycle_release.rs"]
mod synthetic_fixed_cycle_release;

use fn64_boot_harness::{
    parse_unsupported_journal, ClosurePathStatus, ReleaseGateReport, RspRdpObservationKindEvidence,
    PRIVATE_RELEASE_SERIES_COUNT, REPOSITORY_SYNTHETIC_RELEASE_CYCLE,
};
#[cfg(not(feature = "synthetic-native-archive-evidence"))]
use fn64_boot_harness::{
    run_private_release_series, verify_private_release_series,
    verify_repository_synthetic_private_release_run_contract, ExecutionDestinationSource,
    PrivateArtifactIdentity, PrivateChildCommand, PrivateEnvironmentEntry, PrivateFileIdentity,
    PrivateReleaseRunContract, ReleaseRomClass, PRIVATE_RELEASE_RUN_CONTRACT_SCHEMA,
    REPOSITORY_SYNTHETIC_RELEASE_INPUT_BYTES, REPOSITORY_SYNTHETIC_RELEASE_MANIFEST_BYTES,
    REPOSITORY_SYNTHETIC_RELEASE_READINESS_BYTES, REPOSITORY_SYNTHETIC_RELEASE_SCENARIO,
};
#[cfg(feature = "synthetic-native-archive-evidence")]
use fn64_boot_harness::{
    verify_release_matrix, CertificationProfileIdentity, CertificationRequirementClass,
    ReleaseMatrixManifest, ReleaseMatrixScenario, ReleaseMatrixVerification,
    RELEASE_GATE_CYCLE_ENV, RELEASE_MATRIX_SCHEMA, RELEASE_REPORT_ENV, RELEASE_ROM_CLASS_ENV,
    RELEASE_RUN_EVENT_SHA256_ENV,
};
use sha2::{Digest, Sha256};
#[cfg(not(feature = "synthetic-native-archive-evidence"))]
use std::{collections::BTreeSet, fs::File, io::Read};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const CHILD_ENV: &str = "FN64_TEST_PRIVATE_RELEASE_CHILD";
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

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

#[cfg(not(feature = "synthetic-native-archive-evidence"))]
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

#[cfg(not(feature = "synthetic-native-archive-evidence"))]
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
    let output = directory.0.join("series");
    fs::create_dir(&output).unwrap();
    let executable = std::env::current_exe().unwrap();
    for ordinal in 1..=PRIVATE_RELEASE_SERIES_COUNT {
        let report = output.join(format!("report-{ordinal:02}.json"));
        let run_event_sha256 = format!(
            "{:x}",
            Sha256::digest(format!("fn64-public-native-xbus-dpc-run-{ordinal:02}"))
        );
        let child = std::process::Command::new(&executable)
            .current_dir(&directory.0)
            .env_clear()
            .env(CHILD_ENV, "1")
            .env(
                RELEASE_GATE_CYCLE_ENV,
                REPOSITORY_SYNTHETIC_RELEASE_CYCLE.to_string(),
            )
            .env(RELEASE_REPORT_ENV, &report)
            .env(RELEASE_ROM_CLASS_ENV, "unclassified")
            .env(RELEASE_RUN_EVENT_SHA256_ENV, run_event_sha256)
            .args(["--exact", "live_synthetic_child", "--nocapture"])
            .output()
            .unwrap();
        assert!(
            child.status.success(),
            "native XBUS child {ordinal} failed: stdout={} stderr={}",
            String::from_utf8_lossy(&child.stdout),
            String::from_utf8_lossy(&child.stderr)
        );
    }

    let evidence = load_xbus_evidence(&output);
    let semantic_report = &evidence[0].0;
    assert_eq!(
        semantic_report.report_sha256,
        "5fcb34d990ed0e17939028cdbe048ae5888084cf8c8d74c78aa20a95bd1000b4"
    );
    assert_eq!(
        semantic_report.digest.root_sha256,
        "333dc943a42af3e8e1115160201198e12bcc6dc34c9ef712ee63d9c0f1bfc77f"
    );
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
