//! Fresh-process proof that the private-series runner drives the live
//! runtime/device/reference-render release path, not a report-file fixture.

#[allow(dead_code)]
#[path = "../examples/synthetic_fixed_cycle_release.rs"]
mod synthetic_fixed_cycle_release;

use fn64_boot_harness::{
    run_private_release_series, verify_private_release_series,
    verify_repository_synthetic_private_release_run_contract, ExecutionDestinationSource,
    PrivateArtifactIdentity, PrivateChildCommand, PrivateEnvironmentEntry, PrivateFileIdentity,
    PrivateReleaseRunContract, PRIVATE_RELEASE_RUN_CONTRACT_SCHEMA, PRIVATE_RELEASE_SERIES_COUNT,
    REPOSITORY_SYNTHETIC_RELEASE_CYCLE, REPOSITORY_SYNTHETIC_RELEASE_INPUT_BYTES,
    REPOSITORY_SYNTHETIC_RELEASE_MANIFEST_BYTES, REPOSITORY_SYNTHETIC_RELEASE_READINESS_BYTES,
    REPOSITORY_SYNTHETIC_RELEASE_SCENARIO,
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::Read,
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

#[test]
fn live_synthetic_child() {
    if std::env::var_os(CHILD_ENV).is_none() {
        return;
    }
    synthetic_fixed_cycle_release::run_from_release_environment().unwrap();
}

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
        purpose: "synthetic_mechanism".to_owned(),
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
}
