use super::*;
use crate::{ClosurePath, ClosurePathStatus, FixedCycleDigestGate, ReleaseObservationGeometry};
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
const FIXTURE_ENV: &str = "FN64_TEST_RELEASE_CHILD";
const TEMPLATE_ENV: &str = "FN64_TEST_RELEASE_TEMPLATE";

#[test]
fn private_series_tracks_the_current_release_report_schema() {
    assert_eq!(RELEASE_REPORT_SCHEMA, "fn64.release-gate.v29");
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let base = if Path::new("/private/tmp").is_dir() {
            PathBuf::from("/private/tmp")
        } else {
            std::env::temp_dir()
        };
        let path = base.join(format!(
            "fn64-private-release-series-{}-{counter}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn file_identity(path: &Path) -> PrivateFileIdentity {
    let (bytes, sha256) = sha256_file(path, "test file").unwrap();
    PrivateFileIdentity {
        path: path.to_str().unwrap().to_owned(),
        bytes,
        sha256,
    }
}

fn artifact_identity(path: &Path, role: &str) -> PrivateArtifactIdentity {
    let file = file_identity(path);
    PrivateArtifactIdentity {
        role: role.to_owned(),
        path: file.path,
        bytes: file.bytes,
        sha256: file.sha256,
        provenance: "repository_defined_synthetic".to_owned(),
    }
}

fn fixture_report(input: &[u8], source: ExecutionDestinationSource) -> ReleaseGateReport {
    let cycle = REPOSITORY_SYNTHETIC_RELEASE_CYCLE;
    let mut digest = FixedCycleDigestGate::new(cycle);
    digest
        .capture(cycle, ArtifactKind::Framebuffer, &[0, 1])
        .unwrap();
    for kind in [
        ArtifactKind::Audio,
        ArtifactKind::DeviceState,
        ArtifactKind::TimingTrace,
    ] {
        digest.capture(cycle, kind, &[kind as u8]).unwrap();
    }
    digest
        .capture(
            cycle,
            ArtifactKind::Memory,
            &vec![0; crate::DEFAULT_RDRAM_SIZE],
        )
        .unwrap();
    let closure = LIVE_MINIMUM_CLOSURE_PATHS
        .iter()
        .map(|name| ClosurePath {
            name: (*name).to_owned(),
            observations: 1,
            status: ClosurePathStatus::ExercisedZeroUnsupported,
            unsupported: Vec::new(),
        })
        .collect::<Vec<_>>();
    let base = ReleaseGateReport::new(
        REPOSITORY_SYNTHETIC_RELEASE_SCENARIO,
        input,
        digest.finish().unwrap(),
        ReleaseObservationGeometry::reference_rdram(0, 1, 1).unwrap(),
        closure.clone(),
    )
    .unwrap();
    let destinations =
        crate::ExecutionDestinationEvidence::from_ordered(source, Vec::new()).unwrap();
    ReleaseGateReport::new_with_test_environment_and_destinations(
        REPOSITORY_SYNTHETIC_RELEASE_SCENARIO,
        input,
        base.digest,
        base.observations,
        base.environment,
        destinations,
        closure,
    )
    .unwrap()
}

fn fixture_contract(directory: &Path) -> (PrivateReleaseRunContract, PathBuf) {
    fs::create_dir_all(directory).unwrap();
    let manifest = directory.join("manifest.json");
    let readiness = directory.join("readiness.json");
    let input = directory.join("synthetic-input.bin");
    fs::write(&manifest, REPOSITORY_SYNTHETIC_RELEASE_MANIFEST_BYTES).unwrap();
    fs::write(&readiness, REPOSITORY_SYNTHETIC_RELEASE_READINESS_BYTES).unwrap();
    fs::write(&input, REPOSITORY_SYNTHETIC_RELEASE_INPUT_BYTES).unwrap();
    let source = ExecutionDestinationSource::NoProgram;
    let report = fixture_report(&fs::read(&input).unwrap(), source.clone());
    let template = directory.join("report-template.json");
    report.write_json(&template).unwrap();
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
        input: artifact_identity(&input, "synthetic_input"),
        admitted_artifacts: Vec::new(),
        expected_execution_source: source,
        child: PrivateChildCommand {
            executable: file_identity(&executable),
            working_directory: directory.to_str().unwrap().to_owned(),
            argv: vec![
                "--exact".to_owned(),
                "private_release_series::tests::fresh_child_fixture".to_owned(),
                "--nocapture".to_owned(),
            ],
            environment: vec![
                PrivateEnvironmentEntry {
                    name: FIXTURE_ENV.to_owned(),
                    value: "1".to_owned(),
                },
                PrivateEnvironmentEntry {
                    name: TEMPLATE_ENV.to_owned(),
                    value: template.to_str().unwrap().to_owned(),
                },
            ],
        },
        contract_sha256: String::new(),
    };
    contract.contract_sha256 = contract.recompute_contract_sha256().unwrap();
    (contract, template)
}

fn native_fixture_contract(directory: &Path) -> PrivateReleaseRunContract {
    let (mut contract, _) = fixture_contract(directory);
    let generated = directory.join("synthetic-generated.a");
    let bridge = directory.join("synthetic-bridge.a");
    fs::write(&generated, b"repository generated archive").unwrap();
    fs::write(&bridge, b"repository bridge archive").unwrap();
    contract.report_scenario = REPOSITORY_SYNTHETIC_NATIVE_RELEASE_SCENARIO.to_owned();
    contract.admitted_artifacts = vec![
        artifact_identity(&generated, "synthetic_generated_archive"),
        artifact_identity(&bridge, "synthetic_section_bridge_archive"),
    ];
    contract.expected_execution_source = ExecutionDestinationSource::NativeArchive {
        artifact_sha256: hex(&crate::native_program_archives_sha256([
            (
                "synthetic-generated-code".to_owned(),
                fs::read(&generated).unwrap(),
            ),
            (
                "synthetic-section-bridge".to_owned(),
                fs::read(&bridge).unwrap(),
            ),
        ])),
    };
    contract.contract_sha256 = contract.recompute_contract_sha256().unwrap();
    contract
}

#[test]
fn fresh_child_fixture() {
    if std::env::var_os(FIXTURE_ENV).is_none() {
        return;
    }
    let template = PathBuf::from(std::env::var(TEMPLATE_ENV).unwrap());
    let report_path = PathBuf::from(std::env::var(RELEASE_REPORT_ENV).unwrap());
    let cycle = std::env::var(RELEASE_GATE_CYCLE_ENV).unwrap();
    let event = std::env::var(RELEASE_RUN_EVENT_SHA256_ENV).unwrap();
    let report: ReleaseGateReport =
        serde_json::from_slice(&fs::read(&template).unwrap()).unwrap();
    fs::copy(template, &report_path).unwrap();
    let journal = report_path.with_extension("unsupported.jsonl");
    fs::write(
        journal,
        format!(
            "fn64.unsupported-journal.v3\tarmed\t{event}\nfn64.unsupported-journal.v3\tcomplete\t{cycle}\t{}\t{event}\n",
            report.report_sha256
        ),
    )
    .unwrap();
}

#[test]
fn launches_and_reverifies_ten_fresh_child_processes() {
    let directory = TestDirectory::new();
    let (contract, non_runner) = fixture_contract(&directory.0);
    let contract = verify_repository_synthetic_private_release_run_contract(contract).unwrap();
    let output = directory.0.join("series");
    let receipt = run_private_release_series(&contract, &output).unwrap();
    assert_eq!(receipt.count, PRIVATE_RELEASE_SERIES_COUNT);
    assert_eq!(receipt.runs.len(), PRIVATE_RELEASE_SERIES_COUNT);
    assert_eq!(
        receipt
            .runs
            .iter()
            .map(|run| &run.run_event_sha256)
            .collect::<BTreeSet<_>>()
            .len(),
        PRIVATE_RELEASE_SERIES_COUNT
    );
    let verified_series = verify_private_release_series(&contract, &output, &receipt).unwrap();
    verified_series.revalidate_for_release_matrix().unwrap();
    let retained: PrivateReleaseSeriesReceipt =
        serde_json::from_slice(&fs::read(output.join(RECEIPT_FILE)).unwrap()).unwrap();
    assert_eq!(retained, receipt);

    let mut substituted_receipt = receipt.clone();
    substituted_receipt.runner_executable_sha256 = "aa".repeat(32);
    substituted_receipt.receipt_sha256 =
        substituted_receipt.recompute_receipt_sha256().unwrap();
    assert!(verify_private_release_series_with_runner(
        &contract,
        &output,
        &substituted_receipt,
        std::env::current_exe().unwrap(),
    )
    .unwrap_err()
    .to_string()
    .contains("differs from the exact receipt retained"));
    assert!(verify_private_release_series_with_runner(
        &contract,
        &output,
        &receipt,
        &non_runner,
    )
    .unwrap_err()
    .to_string()
    .contains("not a native"));

    fs::write(output.join(report_name(1)), b"retained report drift").unwrap();
    assert!(verified_series
        .revalidate_for_release_matrix()
        .unwrap_err()
        .to_string()
        .contains("parse report"));
}

#[test]
fn child_stage_is_an_exact_independent_inode() {
    let directory = TestDirectory::new();
    let source = directory.0.join("synthetic-native-image");
    fs::write(&source, b"\x7fELFfixed repository bytes").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&source).unwrap().permissions();
        permissions.set_mode(permissions.mode() | 0o700);
        fs::set_permissions(&source, permissions).unwrap();
    }
    let identity = file_identity(&source);
    let staged = stage_child_executable(&identity).unwrap();
    let staged_path = staged.0.clone();
    assert_eq!(fs::read(&staged_path).unwrap(), fs::read(&source).unwrap());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            fs::metadata(&staged_path).unwrap().permissions().mode() & 0o777,
            0o500
        );
    }

    fs::write(&source, b"\x7fELFmutated source bytes").unwrap();
    let staged_identity = PrivateFileIdentity {
        path: staged_path.to_str().unwrap().to_owned(),
        bytes: identity.bytes,
        sha256: identity.sha256,
    };
    verify_file_identity(&staged_identity, "test staged child", false).unwrap();
    drop(staged);
    assert!(!staged_path.exists());
}

#[test]
fn microcode_pair_stage_is_exact_independent_and_revalidated() {
    let directory = TestDirectory::new();
    let text_path = directory.0.join("microcode-text.bin");
    let data_path = directory.0.join("microcode-data.bin");
    fs::write(&text_path, vec![0x5a; fn64_runtime::RSP_MEMORY_BANK_SIZE]).unwrap();
    fs::write(&data_path, b"exact task data").unwrap();

    let (mut contract, _) = fixture_contract(&directory.0.join("contract"));
    contract.purpose = "full_rom".to_owned();
    contract.admitted_artifacts = vec![
        artifact_identity(&data_path, "microcode_data"),
        artifact_identity(&text_path, "microcode_text"),
    ];
    let pair = stage_microcode_pair(&contract).unwrap().unwrap();
    let staged_text_path = pair.text.0.clone();
    let staged_data_path = pair.data.0.clone();
    assert_ne!(staged_text_path, text_path);
    assert_ne!(staged_data_path, data_path);
    assert_eq!(
        fs::read(&staged_text_path).unwrap(),
        vec![0x5a; fn64_runtime::RSP_MEMORY_BANK_SIZE]
    );
    assert_eq!(fs::read(&staged_data_path).unwrap(), b"exact task data");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            fs::metadata(&staged_text_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o400
        );
        assert_eq!(
            fs::metadata(&staged_data_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o400
        );
    }

    fs::write(&text_path, vec![0xa5; fn64_runtime::RSP_MEMORY_BANK_SIZE]).unwrap();
    fs::write(&data_path, b"mutated source").unwrap();
    pair.verify().unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&staged_data_path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    #[cfg(not(unix))]
    {
        let mut permissions = fs::metadata(&staged_data_path).unwrap().permissions();
        permissions.set_readonly(false);
        fs::set_permissions(&staged_data_path, permissions).unwrap();
    }
    fs::write(&staged_data_path, b"tampered staged data").unwrap();
    assert!(pair
        .verify()
        .unwrap_err()
        .to_string()
        .contains("identity drift"));
    drop(pair);
    assert!(!staged_text_path.exists());
    assert!(!staged_data_path.exists());
}

#[test]
fn contract_rejects_shrunk_no_program_and_reserved_environment() {
    let directory = TestDirectory::new();
    let (contract, _) = fixture_contract(&directory.0);

    let mut shrunk = contract.clone();
    shrunk.repeat_count = 9;
    shrunk.contract_sha256 = shrunk.recompute_contract_sha256().unwrap();
    assert!(shrunk
        .verify_integrity()
        .unwrap_err()
        .to_string()
        .contains("exactly 10"));

    let mut no_program = contract.clone();
    no_program.purpose = "full_rom".to_owned();
    no_program.input.role = "rom".to_owned();
    no_program.expected_execution_source = ExecutionDestinationSource::NoProgram;
    no_program.contract_sha256 = no_program.recompute_contract_sha256().unwrap();
    assert!(no_program
        .verify_integrity()
        .unwrap_err()
        .to_string()
        .contains("authoritative executable"));

    for name in [
        "FN64_PRIVATE_RUN_ID",
        RELEASE_MICROCODE_TEXT_PATH_ENV,
        RELEASE_MICROCODE_DATA_PATH_ENV,
        RELEASE_ROM_CLASS_ENV,
    ] {
        let mut reserved = contract.clone();
        reserved.child.environment.push(PrivateEnvironmentEntry {
            name: name.to_owned(),
            value: "forged".to_owned(),
        });
        reserved
            .child
            .environment
            .sort_by(|left, right| left.name.cmp(&right.name));
        reserved.contract_sha256 = reserved.recompute_contract_sha256().unwrap();
        assert!(reserved
            .verify_integrity()
            .unwrap_err()
            .to_string()
            .contains("runner-owned"));
    }
}

#[test]
fn repository_synthetic_authority_and_code_loading_fail_closed() {
    let directory = TestDirectory::new();
    let (contract, _) = fixture_contract(&directory.0);

    let mut relabelled = contract.clone();
    relabelled.rom_class = ReleaseRomClass::RetailCartridge;
    relabelled.contract_sha256 = relabelled.recompute_contract_sha256().unwrap();
    assert!(relabelled
        .verify_integrity()
        .unwrap_err()
        .to_string()
        .contains("must be unclassified"));

    let mut production = contract.clone();
    production.purpose = "full_rom".to_owned();
    production.input.role = "rom".to_owned();
    production.contract_sha256 = production.recompute_contract_sha256().unwrap();
    assert!(verify_release_program_build_receipt_binding(&production)
        .unwrap_err()
        .to_string()
        .contains(RELEASE_PROGRAM_BUILD_RECEIPT_SCHEMA));
    assert!(verify_repository_synthetic_private_release_run_contract(production).is_err());

    let (mut relabelled, _) = fixture_contract(&directory.0.join("relabelled"));
    let input_path = PathBuf::from(&relabelled.input.path);
    fs::write(&input_path, b"caller-labelled non-fixture bytes").unwrap();
    let input = artifact_identity(&input_path, "synthetic_input");
    relabelled.input = input;
    relabelled.contract_sha256 = relabelled.recompute_contract_sha256().unwrap();
    assert!(
        verify_repository_synthetic_private_release_run_contract(relabelled)
            .unwrap_err()
            .to_string()
            .contains("exact repository-defined fixture")
    );

    for name in [
        "PATH",
        "BASH_ENV",
        "SHELLOPTS",
        "GCONV_PATH",
        "SSLKEYLOGFILE",
        "LD_PRELOAD",
        "DYLD_INSERT_LIBRARIES",
        "PYTHONPATH",
        "PERL5LIB",
        "RUBYOPT",
        "NODE_OPTIONS",
        "LUA_PATH",
        "DOTNET_STARTUP_HOOKS",
        "GTK_PATH",
        "QT_PLUGIN_PATH",
        "VK_LAYER_PATH",
        "LIBGL_DRIVERS_PATH",
        "GBM_BACKEND",
        "GALLIUM_DRIVER",
        "EGL_PLATFORM",
        "D3D12SDK_PATH",
        "DXVK_CONFIG_FILE",
        "VKD3D_CONFIG",
    ] {
        let entry = PrivateEnvironmentEntry {
            name: name.to_owned(),
            value: "injected".to_owned(),
        };
        assert!(validate_environment_entry(&entry)
            .unwrap_err()
            .to_string()
            .contains("loader or interpreter"));
    }

    let script = directory.0.join("child-script");
    fs::write(&script, b"#!/bin/sh\nexit 0\n").unwrap();
    assert!(validate_native_executable(&script)
        .unwrap_err()
        .to_string()
        .contains("not a native"));
}

#[test]
fn identified_native_synthetic_series_binds_both_archives() {
    let directory = TestDirectory::new();
    let contract = native_fixture_contract(&directory.0);
    let expected_archives: [PrivateArtifactIdentity; 2] =
        contract.admitted_artifacts.clone().try_into().unwrap();
    let expected_child = contract.child.clone();
    let verify = |candidate| {
        verify_synthetic_native_private_release_run_contract(
            candidate,
            expected_archives.clone(),
            expected_child.clone(),
        )
    };
    verify(contract.clone()).unwrap();

    let mut wrong_identity = contract.clone();
    let ExecutionDestinationSource::NativeArchive { artifact_sha256 } =
        &mut wrong_identity.expected_execution_source
    else {
        panic!("native fixture lost its execution source")
    };
    *artifact_sha256 = "00".repeat(32);
    wrong_identity.contract_sha256 = wrong_identity.recompute_contract_sha256().unwrap();
    assert!(verify(wrong_identity)
        .unwrap_err()
        .to_string()
        .contains("archive identity mismatch"));

    let mut extra = contract.clone();
    let extra_path = directory.0.join("synthetic-extra.a");
    fs::write(&extra_path, b"extra archive").unwrap();
    extra
        .admitted_artifacts
        .push(artifact_identity(&extra_path, "synthetic_z_extra"));
    extra.contract_sha256 = extra.recompute_contract_sha256().unwrap();
    assert!(verify(extra)
        .unwrap_err()
        .to_string()
        .contains("exact caller-bound archives"));

    let mut reordered = contract.clone();
    reordered.admitted_artifacts.reverse();
    reordered.contract_sha256 = reordered.recompute_contract_sha256().unwrap();
    assert!(reordered
        .verify_integrity()
        .unwrap_err()
        .to_string()
        .contains("strictly sorted"));

    let mut relabelled = contract.clone();
    relabelled.report_scenario = REPOSITORY_SYNTHETIC_RELEASE_SCENARIO.to_owned();
    relabelled.contract_sha256 = relabelled.recompute_contract_sha256().unwrap();
    assert!(verify(relabelled).is_err());

    for mutation in ["argv", "environment", "working_directory"] {
        let mut changed = contract.clone();
        match mutation {
            "argv" => changed.child.argv.push("forged".to_owned()),
            "environment" => changed.child.environment[0].value = "forged".to_owned(),
            "working_directory" => changed.child.working_directory.push_str("-forged"),
            _ => unreachable!(),
        }
        changed.contract_sha256 = changed.recompute_contract_sha256().unwrap();
        assert!(verify(changed).is_err());
    }

    let replacement_directory = directory.0.join("replacement");
    fs::create_dir(&replacement_directory).unwrap();
    let mut replaced = native_fixture_contract(&replacement_directory);
    replaced.child = contract.child.clone();
    replaced.contract_sha256 = replaced.recompute_contract_sha256().unwrap();
    assert!(verify(replaced)
        .unwrap_err()
        .to_string()
        .contains("exact caller-bound archives"));

    let mutated = contract;
    let archive = PathBuf::from(&mutated.admitted_artifacts[0].path);
    fs::write(&archive, b"mutated after contract admission").unwrap();
    assert!(verify(mutated)
        .unwrap_err()
        .to_string()
        .contains("admitted_artifacts"));
}

#[test]
fn in_process_admission_rejects_an_invalid_contract() {
    let directory = TestDirectory::new();
    let invalid = directory.0.join("invalid-contract.json");
    fs::write(&invalid, b"{}\n").unwrap();
    assert!(load_private_release_run_contract(&invalid)
        .unwrap_err()
        .to_string()
        .contains("in-process private input admission rejected"));
}

#[cfg(unix)]
#[test]
fn python_emitted_production_contract_authorizes_the_same_build_receipt_in_rust() {
    let directory = TestDirectory::new();
    let manifest_path = directory.0.join("manifest.json");
    let readiness_path = directory.0.join("readiness.json");
    let contract_path = directory.0.join("contract.json");
    let text = directory.0.join("microcode-text.bin");
    let data = directory.0.join("microcode-data.bin");
    let rom = directory.0.join("synthetic-rom.bin");
    let recompiled = directory.0.join("synthetic-recompiled.bin");
    let program_receipt_path = directory.0.join("program-build-receipt.json");
    fs::write(&text, vec![0x5a; 4096]).unwrap();
    fs::write(&data, vec![0xa5; 257]).unwrap();
    fs::write(&rom, b"repository non-game synthetic ROM stand-in").unwrap();
    fs::write(
        &recompiled,
        b"repository non-game synthetic recompiled stand-in",
    )
    .unwrap();
    let executable = Path::new("/usr/bin/true").canonicalize().unwrap();
    let recompiled_identity = file_identity(&recompiled);
    let executable_identity = file_identity(&executable);
    let expected_execution_source = ExecutionDestinationSource::TypedBlockProgram {
        program_sha256: "11".repeat(32),
        dispatch_artifact_sha256: recompiled_identity.sha256.clone(),
    };
    let mut program_receipt = ReleaseProgramBuildReceipt {
        schema: RELEASE_PROGRAM_BUILD_RECEIPT_SCHEMA.to_owned(),
        child_executable: ReleaseProgramFileIdentity {
            path: executable_identity.path.clone(),
            bytes: executable_identity.bytes,
            sha256: executable_identity.sha256.clone(),
        },
        lane: ReleaseProgramBuildLane::TypedBlock {
            pack: ReleaseProgramFileIdentity {
                path: recompiled_identity.path.clone(),
                bytes: recompiled_identity.bytes,
                sha256: recompiled_identity.sha256.clone(),
            },
            expected_program_sha256: "11".repeat(32),
        },
        expected_execution_source: expected_execution_source.clone(),
        receipt_sha256: String::new(),
    };
    program_receipt.receipt_sha256 = program_receipt.recompute_receipt_sha256().unwrap();
    fs::write(
        &program_receipt_path,
        serde_json::to_vec_pretty(&program_receipt).unwrap(),
    )
    .unwrap();

    let descriptor = |path: &Path, provenance: &str| {
        let identity = file_identity(path);
        serde_json::json!({
            "path": identity.path,
            "length": identity.bytes,
            "sha256": identity.sha256,
            "provenance": provenance,
            "git_identity": "excluded",
        })
    };
    let program_receipt_identity = file_identity(&program_receipt_path);
    let manifest = serde_json::json!({
        "schema": "fn64.private-input-admission.v7",
        "purpose": "full_rom",
        "intent": {
            "wire_family": "full_rom_mixed",
            "report_scenario": "synthetic-python-rust-policy-parity",
            "recognition": "runtime_must_confirm_backend_known_pair",
            "extended_gbi_cases": [],
            "characterization_suite": null,
            "program_evidence_lane": "typed_block_program",
            "rom_class": "retail_cartridge",
        },
        "release_matrix": {
            "platform": "macos_arm64",
            "controllers": ["standard_controller"],
            "save": "no_cartridge_save",
            "renderers": ["reference_lle_accuracy"],
            "repeat_bar": 10,
        },
        "artifacts": {
            "microcode_text": descriptor(&text, "user_owned_rom_derived"),
            "microcode_data": descriptor(&data, "user_owned_rom_derived"),
            "microcode_text_raw_window": null,
            "microcode_data_raw_window": null,
            "rom": descriptor(&rom, "user_owned_retail_cartridge_dump"),
            "recompiled": descriptor(&recompiled, "user_generated_from_owned_rom"),
        },
        "runner": {
            "executable": {
                "path": executable_identity.path,
                "length": executable_identity.bytes,
                "sha256": executable_identity.sha256,
                "git_identity": "excluded",
            },
            "working_directory": directory.0.to_str().unwrap(),
            "argv": ["--exact", "unused-private-release-child"],
            "env": {"FN64_SYNTHETIC_FIXED": "1"},
            "release_gate_cycle": REPOSITORY_SYNTHETIC_RELEASE_CYCLE,
            "execution_source": {
                "kind": "typed_block_program",
                "program_sha256": "11".repeat(32),
                "dispatch_artifact_sha256": recompiled_identity.sha256,
            },
            "program_build_receipt": {
                "path": program_receipt_identity.path,
                "length": program_receipt_identity.bytes,
                "sha256": program_receipt_identity.sha256,
                "git_identity": "excluded",
            },
        },
    });
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    let script =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/private_input_admission.py");
    let status = Command::new("/usr/bin/python3")
        .arg(&script)
        .arg("--manifest")
        .arg(&manifest_path)
        .arg("--report")
        .arg(&readiness_path)
        .arg("--emit-private-run-contract")
        .arg(&contract_path)
        .status()
        .unwrap();
    assert!(status.success());

    load_private_release_run_contract(&contract_path).unwrap();
}

#[test]
fn production_report_requires_one_recognized_event_with_the_exact_admitted_pair() {
    let directory = TestDirectory::new();
    let (mut contract, _) = fixture_contract(&directory.0);
    contract.purpose = "full_rom".to_owned();
    contract.input.role = "rom".to_owned();
    contract.admitted_artifacts = vec![
        PrivateArtifactIdentity {
            role: "microcode_data".to_owned(),
            path: "/private/ucode.data".to_owned(),
            bytes: 257,
            sha256: "22".repeat(32),
            provenance: "user_owned_rom_derived".to_owned(),
        },
        PrivateArtifactIdentity {
            role: "microcode_text".to_owned(),
            path: "/private/ucode.text".to_owned(),
            bytes: fn64_runtime::RSP_MEMORY_BANK_SIZE as u64,
            sha256: "11".repeat(32),
            provenance: "user_owned_rom_derived".to_owned(),
        },
        PrivateArtifactIdentity {
            role: "recompiled".to_owned(),
            path: "/private/program.pack".to_owned(),
            bytes: 1,
            sha256: "33".repeat(32),
            provenance: "user_generated_from_owned_rom".to_owned(),
        },
    ];
    let mut report = fixture_report(b"ignored", ExecutionDestinationSource::NoProgram);
    report.rsp_rdp =
        crate::RspRdpEvidence::from_ordered(vec![crate::RspRdpObservationEventEvidence {
            guest_cycle: 42,
            observation: RspRdpObservationKindEvidence::MicrocodeRecognition {
                task_address: 0x1000,
                imem_generation: 1,
                text_sha256: "11".repeat(32),
                data_address: 0x2000,
                data_bytes: 257,
                data_sha256: "22".repeat(32),
                family: Some(crate::ReleaseMicrocodeFamily::F3dzex2),
            },
        }])
        .unwrap();
    verify_consumed_microcode_pair(&contract, 1, &report).unwrap();

    let matching = report.rsp_rdp.ordered[0].clone();
    let mut split = report.clone();
    let mut text_only = matching.clone();
    let RspRdpObservationKindEvidence::MicrocodeRecognition { data_sha256, .. } =
        &mut text_only.observation
    else {
        unreachable!()
    };
    *data_sha256 = "44".repeat(32);
    let mut data_only = matching;
    let RspRdpObservationKindEvidence::MicrocodeRecognition { text_sha256, .. } =
        &mut data_only.observation
    else {
        unreachable!()
    };
    *text_sha256 = "55".repeat(32);
    split.rsp_rdp = crate::RspRdpEvidence::from_ordered(vec![text_only, data_only]).unwrap();
    assert!(verify_consumed_microcode_pair(&contract, 2, &split)
        .unwrap_err()
        .to_string()
        .contains("no single recognized microcode event"));

    let RspRdpObservationKindEvidence::MicrocodeRecognition { family, .. } =
        &mut report.rsp_rdp.ordered[0].observation
    else {
        unreachable!()
    };
    *family = None;
    assert!(verify_consumed_microcode_pair(&contract, 3, &report).is_err());
}

#[test]
fn production_report_rom_binding_rejects_class_length_and_input_relabels() {
    let directory = TestDirectory::new();
    let (mut contract, _) = fixture_contract(&directory.0);
    let mut rom_bytes = vec![0u8; 0x40];
    rom_bytes[..4].copy_from_slice(&[0x80, 0x37, 0x12, 0x40]);
    rom_bytes[0x3e] = b'E';
    contract.purpose = "full_rom".to_owned();
    contract.rom_class = ReleaseRomClass::RetailCartridge;
    contract.input.role = "rom".to_owned();
    contract.input.bytes = rom_bytes.len() as u64;
    contract.input.sha256 = sha256_hex(&rom_bytes);
    contract.input.provenance = "user_owned_retail_cartridge_dump".to_owned();
    fs::write(&contract.input.path, &rom_bytes).unwrap();

    let mut report = fixture_report(&rom_bytes, ExecutionDestinationSource::NoProgram);
    report.rom = Some(
        crate::ReleaseRomEvidence::from_bytes(
            &rom_bytes,
            ReleaseRomClass::RetailCartridge,
            fn64_runtime::TvType::Ntsc,
        )
        .unwrap(),
    );
    verify_report_rom_binding(&contract, 1, &report).unwrap();

    let mut relabelled = report.clone();
    relabelled.rom.as_mut().unwrap().class = ReleaseRomClass::PublicHomebrew;
    assert!(verify_report_rom_binding(&contract, 2, &relabelled)
        .unwrap_err()
        .to_string()
        .contains("class/length"));

    let mut resized = report.clone();
    resized.rom.as_mut().unwrap().byte_len += 4;
    assert!(verify_report_rom_binding(&contract, 3, &resized).is_err());

    let mut forged_header_identity = report.clone();
    forged_header_identity
        .rom
        .as_mut()
        .unwrap()
        .canonical_sha256 = "11".repeat(32);
    assert!(
        verify_report_rom_binding(&contract, 4, &forged_header_identity)
            .unwrap_err()
            .to_string()
            .contains("independently decoded")
    );

    let mut other_input = report;
    other_input.input_sha256 = "00".repeat(32);
    assert!(verify_report_rom_binding(&contract, 5, &other_input)
        .unwrap_err()
        .to_string()
        .contains("input SHA-256"));
}

#[test]
fn canonical_wires_bind_order_context_and_receipt_tamper() {
    let directory = TestDirectory::new();
    let (contract, _) = fixture_contract(&directory.0);
    let baseline = contract.recompute_contract_sha256().unwrap();
    let mut changed = contract.clone();
    changed.child.argv.push("changed".to_owned());
    assert_ne!(changed.recompute_contract_sha256().unwrap(), baseline);
    let mut relabelled = contract.clone();
    relabelled.rom_class = ReleaseRomClass::RetailCartridge;
    assert_ne!(relabelled.recompute_contract_sha256().unwrap(), baseline);

    let nonce = [0x5a; 32];
    let first = derive_run_event_sha256(
        &nonce,
        &contract.contract_sha256,
        &contract.child.executable.sha256,
        1,
        "report-01.json",
    )
    .unwrap();
    let second = derive_run_event_sha256(
        &nonce,
        &contract.contract_sha256,
        &contract.child.executable.sha256,
        2,
        "report-02.json",
    )
    .unwrap();
    assert_ne!(first, second);

    let runs = (1..=PRIVATE_RELEASE_SERIES_COUNT)
        .map(|ordinal| PrivateReleaseSeriesRun {
            ordinal: ordinal as u64,
            run_event_sha256: format!("{ordinal:064x}"),
            report_file_sha256: "22".repeat(32),
            journal_file_sha256: "33".repeat(32),
            report_sha256: "44".repeat(32),
            artifact_root_sha256: "55".repeat(32),
        })
        .collect();
    let mut receipt = PrivateReleaseSeriesReceipt {
        schema: PRIVATE_RELEASE_SERIES_RECEIPT_SCHEMA.to_owned(),
        contract_sha256: contract.contract_sha256,
        runner_executable_sha256: "66".repeat(32),
        child_executable_sha256: contract.child.executable.sha256,
        series_nonce: "77".repeat(32),
        report_scenario: contract.report_scenario,
        input_sha256: contract.input.sha256,
        guest_cycle: contract.guest_cycle,
        expected_execution_source: contract.expected_execution_source,
        count: PRIVATE_RELEASE_SERIES_COUNT,
        semantic_report_sha256: "44".repeat(32),
        runs,
        receipt_sha256: String::new(),
    };
    receipt.receipt_sha256 = receipt.recompute_receipt_sha256().unwrap();
    receipt.verify_integrity().unwrap();
    receipt.runs.swap(0, 1);
    assert!(receipt.verify_integrity().is_err());
}

#[test]
fn contract_wire_matches_cross_language_golden() {
    let contract = PrivateReleaseRunContract {
        schema: PRIVATE_RELEASE_RUN_CONTRACT_SCHEMA.to_owned(),
        admission_manifest: PrivateFileIdentity {
            path: "/private/manifest.json".to_owned(),
            bytes: 123,
            sha256: "00".repeat(32),
        },
        readiness_report: PrivateFileIdentity {
            path: "/private/readiness.json".to_owned(),
            bytes: 456,
            sha256: "11".repeat(32),
        },
        program_build_receipt: Some(PrivateFileIdentity {
            path: "/private/program-build-receipt.json".to_owned(),
            bytes: 654,
            sha256: "12".repeat(32),
        }),
        purpose: "full_rom".to_owned(),
        rom_class: ReleaseRomClass::RetailCartridge,
        report_scenario: "canonical-wire-fixture".to_owned(),
        guest_cycle: 42,
        repeat_count: 10,
        input: PrivateArtifactIdentity {
            role: "rom".to_owned(),
            path: "/private/game.z64".to_owned(),
            bytes: 67_108_864,
            sha256: "22".repeat(32),
            provenance: "user_owned_retail_cartridge_dump".to_owned(),
        },
        admitted_artifacts: vec![
            PrivateArtifactIdentity {
                role: "microcode_data".to_owned(),
                path: "/private/ucode.data".to_owned(),
                bytes: 128,
                sha256: "33".repeat(32),
                provenance: "user_owned_rom_derived".to_owned(),
            },
            PrivateArtifactIdentity {
                role: "microcode_text".to_owned(),
                path: "/private/ucode.text".to_owned(),
                bytes: fn64_runtime::RSP_MEMORY_BANK_SIZE as u64,
                sha256: "34".repeat(32),
                provenance: "user_owned_rom_derived".to_owned(),
            },
            PrivateArtifactIdentity {
                role: "recompiled".to_owned(),
                path: "/private/game.a".to_owned(),
                bytes: 789,
                sha256: "44".repeat(32),
                provenance: "user_generated_from_owned_rom".to_owned(),
            },
        ],
        expected_execution_source: ExecutionDestinationSource::TypedBlockProgram {
            program_sha256: "55".repeat(32),
            dispatch_artifact_sha256: "66".repeat(32),
        },
        child: PrivateChildCommand {
            executable: PrivateFileIdentity {
                path: "/private/game".to_owned(),
                bytes: 999,
                sha256: "77".repeat(32),
            },
            working_directory: "/private/run".to_owned(),
            argv: vec!["--headless".to_owned(), "value".to_owned()],
            environment: vec![
                PrivateEnvironmentEntry {
                    name: "A_FIXED".to_owned(),
                    value: "1".to_owned(),
                },
                PrivateEnvironmentEntry {
                    name: "Z_FIXED".to_owned(),
                    value: "two".to_owned(),
                },
            ],
        },
        contract_sha256: String::new(),
    };
    assert_eq!(
        contract.recompute_contract_sha256().unwrap(),
        "e4ca4cf7a3a6beaf88515ffc04d235c74fabf63f8d99cec5f20cb359a13712b3"
    );
}

#[test]
fn file_identity_drift_and_contract_digest_tamper_fail_closed() {
    let directory = TestDirectory::new();
    let (mut contract, _) = fixture_contract(&directory.0);
    contract.contract_sha256 = "00".repeat(32);
    assert!(contract
        .verify_integrity()
        .unwrap_err()
        .to_string()
        .contains("mismatch"));

    let (contract, _) = fixture_contract(&directory.0.join("other"));
    fs::write(&contract.input.path, b"mutated synthetic input").unwrap();
    assert!(contract
        .verify_bound_files()
        .unwrap_err()
        .to_string()
        .contains("identity drift"));
}
