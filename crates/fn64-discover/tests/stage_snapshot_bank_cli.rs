use fn64_discover::facts::{
    executable_range_subject, function_entry_subject, CandidateDetector, FunctionEntryEvidence,
    ProloguePattern,
};
use fn64_discover::snapshot::{
    compose_materialized_bank_v1, MaterializedBankInput, ProgramSnapshotV1,
};
use fn64_discover::tool_adapter::Sha256Digest;
use fn64_discover::tool_claims::program_snapshot_sha256_v2;
use fn64_discover::{normalize, BankAddr, Fact, FactDb, ProofState, RomAddressSpace};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

const BASE: u32 = 0x8000_0400;
const EXTRA: u32 = BASE + 0x10;
const ROM_START: u32 = 0x1000;

struct Fixture {
    root: PathBuf,
    workspace: PathBuf,
    snapshot: PathBuf,
    bank: PathBuf,
    bank_bytes: Vec<u8>,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "fn64-stage-snapshot-bank-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let workspace = root.join("workspace");
        fs::create_dir(&workspace).unwrap();
        #[cfg(unix)]
        fs::set_permissions(&workspace, fs::Permissions::from_mode(0o700)).unwrap();
        let workspace = fs::canonicalize(workspace).unwrap();

        let (snapshot_value, bank_bytes) = snapshot();
        let snapshot_path = root.join("snapshot.json");
        fs::write(&snapshot_path, serde_json::to_vec(&snapshot_value).unwrap()).unwrap();
        let bank_path = root.join("bank.bin");
        fs::write(&bank_path, &bank_bytes).unwrap();
        Self {
            root,
            workspace,
            snapshot: snapshot_path,
            bank: bank_path,
            bank_bytes,
        }
    }

    fn run(&self, source: &Path, out_bank: &Path, out_evidence: &Path) -> Output {
        self.run_with_seeds(source, out_bank, out_evidence, BASE, EXTRA)
    }

    fn run_with_seeds(
        &self,
        source: &Path,
        out_bank: &Path,
        out_evidence: &Path,
        base_seed: u32,
        snapshot_seed: u32,
    ) -> Output {
        Command::new(env!("CARGO_BIN_EXE_stage_snapshot_bank"))
            .arg(&self.snapshot)
            .arg("boot")
            .arg(source)
            .arg(&self.workspace)
            .arg(out_bank)
            .arg(out_evidence)
            .arg(format!("0x{base_seed:08x}"))
            .arg(format!("0x{snapshot_seed:08x}"))
            .output()
            .unwrap()
    }

    fn run_base_only(
        &self,
        source: &Path,
        out_bank: &Path,
        out_evidence: &Path,
        base_seed: u32,
    ) -> Output {
        Command::new(env!("CARGO_BIN_EXE_stage_snapshot_bank"))
            .arg("--base-only")
            .arg(&self.snapshot)
            .arg("boot")
            .arg(source)
            .arg(&self.workspace)
            .arg(out_bank)
            .arg(out_evidence)
            .arg(format!("0x{base_seed:08x}"))
            .output()
            .unwrap()
    }

    fn run_discovery_only(&self, source: &Path, out_bank: &Path, out_evidence: &Path) -> Output {
        Command::new(env!("CARGO_BIN_EXE_stage_snapshot_bank"))
            .arg("--discovery-only")
            .arg(&self.snapshot)
            .arg("boot")
            .arg(source)
            .arg(&self.workspace)
            .arg(out_bank)
            .arg(out_evidence)
            .output()
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

fn snapshot() -> (ProgramSnapshotV1, Vec<u8>) {
    let mut bytes = vec![0u8; 0x1020];
    bytes[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
    bytes[8..12].copy_from_slice(&BASE.to_be_bytes());
    for (offset, word) in [
        (0x1000, 0x0c00_0104u32),
        (0x1004, 0),
        (0x1008, 0x03e0_0008),
        (0x100c, 0),
        (0x1010, 0x03e0_0008),
        (0x1014, 0),
    ] {
        bytes[offset..offset + 4].copy_from_slice(&word.to_be_bytes());
    }
    let rom = normalize(&bytes).unwrap();
    let bank_bytes = rom.bytes[ROM_START as usize..].to_vec();
    let mut facts = FactDb::new();
    let mapping = facts.insert(Fact::RomMapping {
        bank: "boot".into(),
        rom_space: RomAddressSpace::Physical,
        rom_start: ROM_START,
        rom_end: ROM_START + bank_bytes.len() as u32,
        va_start: BASE,
        va_end: BASE + bank_bytes.len() as u32,
    });
    facts
        .conclude("bank:boot", ProofState::Proven, vec![mapping], "test")
        .unwrap();
    let executable = facts.insert(Fact::ExecutableRange {
        bank: "boot".into(),
        va_start: BASE,
        va_end: BASE + bank_bytes.len() as u32,
    });
    facts
        .conclude(
            executable_range_subject("boot", BASE, BASE + bank_bytes.len() as u32),
            ProofState::Proven,
            vec![executable],
            "test",
        )
        .unwrap();
    for entry in [BASE, EXTRA] {
        let target = BankAddr::new("boot", entry);
        let claim = facts.insert(Fact::FunctionEntryClaim {
            target: target.clone(),
            detector: CandidateDetector::ProloguePattern,
            evidence: FunctionEntryEvidence::Prologue {
                stack_adjust: target.clone(),
                frame_size: 16,
                pattern: ProloguePattern::LeafWithMatchedRestore,
                corroborating_site: BankAddr::new("boot", entry + 4),
            },
            proposed_state: ProofState::Proven,
        });
        facts
            .conclude(
                function_entry_subject(&target),
                ProofState::Proven,
                vec![claim],
                "test",
            )
            .unwrap();
    }
    let snapshot = compose_materialized_bank_v1(
        &rom,
        &facts,
        MaterializedBankInput {
            bank: "boot",
            va_start: BASE,
            bytes: &bank_bytes,
            seed_roots: &[BASE, EXTRA],
        },
    )
    .unwrap();
    (snapshot, bank_bytes)
}

#[test]
fn stages_exact_bytes_and_path_free_deterministic_evidence() {
    let fixture = Fixture::new("success");
    let out_bank = fixture.workspace.join("staged.bin");
    let out_evidence = fixture.workspace.join("evidence.json");
    let output = fixture.run(&fixture.bank, &out_bank, &out_evidence);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read(&out_bank).unwrap(), fixture.bank_bytes);
    let evidence_bytes = fs::read(&out_evidence).unwrap();
    assert_eq!(evidence_bytes.last(), Some(&b'\n'));
    let evidence: serde_json::Value = serde_json::from_slice(&evidence_bytes).unwrap();
    assert_eq!(evidence["schema"], "fn64.snapshot-bank-evidence");
    assert_eq!(evidence["schema_version"], 2);
    assert_eq!(evidence["input"]["bank"], "boot");
    assert_eq!(evidence["input"]["va_start"], BASE);
    assert_eq!(evidence["input"]["va_end"], BASE + 0x20);
    assert_eq!(evidence["artifact"]["byte_length"], 0x20);
    assert_eq!(evidence["seeds"]["mode"], "paired");
    assert_eq!(evidence["seeds"]["base_seed"], BASE);
    assert_eq!(evidence["seeds"]["snapshot_seed"], EXTRA);
    assert!(!String::from_utf8(evidence_bytes)
        .unwrap()
        .contains(fixture.root.to_str().unwrap()));

    let second_bank = fixture.workspace.join("staged-second.bin");
    let second_evidence = fixture.workspace.join("evidence-second.json");
    let second = fixture.run(&fixture.bank, &second_bank, &second_evidence);
    assert!(second.status.success());
    assert_eq!(
        fs::read(&out_evidence).unwrap(),
        fs::read(&second_evidence).unwrap()
    );

    #[cfg(unix)]
    {
        assert_eq!(
            fs::metadata(out_bank).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(out_evidence).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn base_only_stages_with_an_explicitly_absent_snapshot_seed() {
    let fixture = Fixture::new("base-only");
    let out_bank = fixture.workspace.join("base-only.bin");
    let out_evidence = fixture.workspace.join("base-only-evidence.json");
    let output = fixture.run_base_only(&fixture.bank, &out_bank, &out_evidence, BASE);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read(out_bank).unwrap(), fixture.bank_bytes);
    let evidence: serde_json::Value =
        serde_json::from_slice(&fs::read(out_evidence).unwrap()).unwrap();
    assert_eq!(evidence["schema_version"], 2);
    assert_eq!(evidence["seeds"]["mode"], "base_only");
    assert_eq!(evidence["seeds"]["base_seed"], BASE);
    assert!(evidence["seeds"].get("snapshot_seed").is_none());
}

#[test]
fn discovery_only_stages_verified_bytes_without_manufacturing_a_seed() {
    let fixture = Fixture::new("discovery-only");
    let out_bank = fixture.workspace.join("discovery-only.bin");
    let out_evidence = fixture.workspace.join("discovery-only-evidence.json");
    let output = fixture.run_discovery_only(&fixture.bank, &out_bank, &out_evidence);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read(out_bank).unwrap(), fixture.bank_bytes);
    let evidence: serde_json::Value =
        serde_json::from_slice(&fs::read(out_evidence).unwrap()).unwrap();
    assert_eq!(evidence["schema_version"], 3);
    assert_eq!(evidence["seeds"]["mode"], "discovery_only");
    assert_eq!(evidence["seeds"]["role"], "candidate_only");
    assert!(evidence["seeds"].get("base_seed").is_none());
    assert!(evidence["seeds"].get("snapshot_seed").is_none());
}

#[test]
fn discovery_only_rejects_an_accidental_seed_argument() {
    let fixture = Fixture::new("discovery-only-extra");
    let output = Command::new(env!("CARGO_BIN_EXE_stage_snapshot_bank"))
        .arg("--discovery-only")
        .arg(&fixture.snapshot)
        .arg("boot")
        .arg(&fixture.bank)
        .arg(&fixture.workspace)
        .arg(fixture.workspace.join("discovery-only.bin"))
        .arg(fixture.workspace.join("discovery-only-evidence.json"))
        .arg(format!("0x{BASE:08x}"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("usage:"));
}

#[test]
fn base_only_rejects_a_base_that_is_not_a_proven_owner() {
    let fixture = Fixture::new("base-only-seed");
    let output = fixture.run_base_only(
        &fixture.bank,
        &fixture.workspace.join("base-only.bin"),
        &fixture.workspace.join("base-only-evidence.json"),
        BASE + 0x1c,
    );
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("base seed is not a proven owner entry")
    );
}

#[test]
fn base_only_rejects_an_accidental_snapshot_seed_argument() {
    let fixture = Fixture::new("base-only-extra");
    let output = Command::new(env!("CARGO_BIN_EXE_stage_snapshot_bank"))
        .arg("--base-only")
        .arg(&fixture.snapshot)
        .arg("boot")
        .arg(&fixture.bank)
        .arg(&fixture.workspace)
        .arg(fixture.workspace.join("base-only.bin"))
        .arg(fixture.workspace.join("base-only-evidence.json"))
        .arg(format!("0x{BASE:08x}"))
        .arg(format!("0x{EXTRA:08x}"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("usage:"));
}

#[test]
fn rejects_wrong_length_and_non_snapshot_seeds() {
    let fixture = Fixture::new("geometry-seeds");
    let short = fixture.root.join("short.bin");
    fs::write(&short, &fixture.bank_bytes[..fixture.bank_bytes.len() - 4]).unwrap();
    let output = fixture.run(
        &short,
        &fixture.workspace.join("short-staged.bin"),
        &fixture.workspace.join("short-evidence.json"),
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("does not match snapshot length"));

    let output = fixture.run_with_seeds(
        &fixture.bank,
        &fixture.workspace.join("seed-staged.bin"),
        &fixture.workspace.join("seed-evidence.json"),
        BASE,
        BASE + 0x1c,
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("snapshot seed is not an assessed owner entry"));
}

#[test]
fn rejects_changed_bytes_and_existing_outputs_without_publication() {
    let fixture = Fixture::new("reject");
    let changed = fixture.root.join("changed.bin");
    let mut bytes = fixture.bank_bytes.clone();
    bytes[0] ^= 1;
    fs::write(&changed, bytes).unwrap();
    let out_bank = fixture.workspace.join("staged.bin");
    let out_evidence = fixture.workspace.join("evidence.json");
    let output = fixture.run(&changed, &out_bank, &out_evidence);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("digest does not match"));
    assert!(!out_bank.exists());
    assert!(!out_evidence.exists());

    fs::write(&out_evidence, b"winner").unwrap();
    let output = fixture.run(&fixture.bank, &out_bank, &out_evidence);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("refusing to overwrite"));
    assert!(!out_bank.exists());
    assert_eq!(fs::read(out_evidence).unwrap(), b"winner");
}

#[cfg(unix)]
#[test]
fn rejects_symlinked_input_and_output_parent() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new("symlink");
    let source_alias = fixture.root.join("bank-alias.bin");
    symlink(&fixture.bank, &source_alias).unwrap();
    let out_bank = fixture.workspace.join("staged.bin");
    let out_evidence = fixture.workspace.join("evidence.json");
    let output = fixture.run(&source_alias, &out_bank, &out_evidence);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not a regular file"));

    let real = fixture.workspace.join("real");
    fs::create_dir(&real).unwrap();
    let alias = fixture.workspace.join("alias");
    symlink(&real, &alias).unwrap();
    let output = fixture.run(
        &fixture.bank,
        &alias.join("staged.bin"),
        &alias.join("evidence.json"),
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("output directory must be canonical"));
}

#[cfg(unix)]
#[test]
fn snapshot_bank_runner_distinguishes_base_only_and_paired_staging() {
    fn make_executable(path: &Path, body: &str) {
        fs::write(path, body).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }

    fn retained_diagnostics(workspace: &Path) -> String {
        let mut diagnostics = String::new();
        for attempt in fs::read_dir(workspace).unwrap() {
            let attempt = attempt.unwrap().path();
            let directory = attempt.join("diagnostics");
            let Ok(entries) = fs::read_dir(directory) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if let Ok(bytes) = fs::read(&path) {
                    diagnostics.push_str(&format!(
                        "\n{}:\n{}",
                        path.display(),
                        String::from_utf8_lossy(&bytes)
                    ));
                }
            }
        }
        diagnostics
    }

    let fixture = Fixture::new("runner-base-only");
    let ghidra = fixture.root.join("ghidra");
    let support = ghidra.join("support");
    let application = ghidra.join("Ghidra");
    fs::create_dir_all(&support).unwrap();
    fs::create_dir_all(&application).unwrap();
    fs::write(
        application.join("application.properties"),
        b"application.version=synthetic\n",
    )
    .unwrap();
    let headless_script = r#"#!/bin/sh
set -eu
provider=
for argument in "$@"; do
    case "$argument" in
        */provider.jsonl) provider=$argument ;;
    esac
done
[ -n "$provider" ]
printf '{}\n' > "$provider"
echo 'Using Loader: Raw Binary'
echo 'Using Language/Compiler: MIPS:BE:64:64-32addr:o32'
"#;
    make_executable(&support.join("analyzeHeadless"), headless_script);

    let jdk = fixture.root.join("jdk");
    fs::create_dir_all(jdk.join("bin")).unwrap();
    make_executable(&jdk.join("bin/java"), "#!/bin/sh\nexit 0\n");

    let (snapshot, _) = snapshot();
    let snapshot_sha = program_snapshot_sha256_v2(&snapshot).unwrap().to_hex();
    let ingest = fixture.root.join("fake-ingest");
    let ingest_script = r#"#!/bin/sh
set -eu
python3 - "$2" "$4" <<'PY'
import json
import sys

request_path, output_path = sys.argv[1:]
with open(request_path, "r", encoding="utf-8") as stream:
    request = json.load(stream)
value = {
    "schema": "fn64.tool-claim-set",
    "schema_version": 1,
    "program_snapshot_sha256": "SNAPSHOT_SHA",
    "sources": [{"tool": {"name": run["tool"]["name"]}} for run in request["runs"]],
    "claims": [{}],
}
with open(output_path, "x", encoding="utf-8") as stream:
    stream.write(json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n")
PY
echo 'ingest-tool-claims: snapshot=SNAPSHOT_SHA'
"#
    .replace("SNAPSHOT_SHA", &snapshot_sha);
    make_executable(&ingest, &ingest_script);

    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let output = Command::new(repo.join("tools/ghidra/run-snapshot-bank.sh"))
        .arg("--unseeded-only")
        .arg(&fixture.snapshot)
        .arg("boot")
        .arg(&fixture.bank)
        .arg(&fixture.workspace)
        .arg(format!("0x{BASE:08x}"))
        .env(
            "FN64_STAGE_SNAPSHOT_BANK",
            env!("CARGO_BIN_EXE_stage_snapshot_bank"),
        )
        .env("FN64_INGEST_TOOL_CLAIMS", &ingest)
        .env("GHIDRA_INSTALL_DIR", &ghidra)
        .env("GHIDRA_JAVA_HOME", &jdk)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}\ndiagnostics:{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        retained_diagnostics(&fixture.workspace)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let attempt = stdout
        .lines()
        .find_map(|line| line.strip_prefix("attempt="))
        .map(PathBuf::from)
        .expect("runner did not print its attempt path");

    let evidence: serde_json::Value =
        serde_json::from_slice(&fs::read(attempt.join("raw/evidence.json")).unwrap()).unwrap();
    assert_eq!(evidence["seeds"]["mode"], "base_only");
    assert_eq!(evidence["seeds"]["base_seed"], BASE);
    assert!(evidence["seeds"].get("snapshot_seed").is_none());

    let config: serde_json::Value =
        serde_json::from_slice(&fs::read(attempt.join("config/unseeded.json")).unwrap()).unwrap();
    assert_eq!(config["base_seed"], BASE);
    assert_eq!(config["snapshot_seed"], serde_json::Value::Null);

    let receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(attempt.join("out/receipt.json")).unwrap()).unwrap();
    assert_eq!(receipt["execution_mode"], "unseeded-only");
    assert_eq!(receipt["paired_comparison_complete"], false);
    assert_eq!(receipt["seeds"]["mode"], "base_only");
    assert_eq!(receipt["seeds"]["base_seed"], BASE);
    assert!(receipt["seeds"].get("snapshot_seed").is_none());
    assert!(receipt.get("seeded_tool_manifest_sha256").is_none());
    assert!(!attempt.join("config/seeded.json").exists());
    assert!(!attempt.join("tool-seeded.json").exists());
    assert!(!attempt.join("modes/seeded").exists());

    let tool_manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(attempt.join("tool-unseeded.json")).unwrap()).unwrap();
    let artifacts = tool_manifest["artifacts"].as_array().unwrap();
    let artifact_paths: Vec<_> = artifacts
        .iter()
        .map(|artifact| artifact["path"].as_str().unwrap())
        .collect();
    assert_eq!(
        artifact_paths,
        vec![
            "tool-artifacts/Fn64ExportCandidates.java",
            "tool-artifacts/Fn64SeedFunctions.java",
            "tool-artifacts/analyzeHeadless",
            "tool-artifacts/application.properties",
            "tool-artifacts/ghidra-distribution.json",
            "tool-artifacts/java",
            "tool-artifacts/orchestration.json",
        ]
    );
    let orchestration: serde_json::Value = serde_json::from_slice(
        &fs::read(attempt.join("tool-artifacts/orchestration.json")).unwrap(),
    )
    .unwrap();
    let orchestration_bytes = fs::read(attempt.join("tool-artifacts/orchestration.json")).unwrap();
    let orchestration_entry = artifacts
        .iter()
        .find(|artifact| artifact["path"] == "tool-artifacts/orchestration.json")
        .unwrap();
    assert_eq!(
        orchestration_entry["sha256"],
        Sha256Digest::of(&orchestration_bytes).to_hex()
    );
    assert_eq!(
        orchestration["schema"],
        "fn64.ghidra-orchestration-artifacts"
    );
    let orchestration_artifacts = orchestration["artifacts"].as_array().unwrap();
    let orchestration_paths: Vec<_> = orchestration_artifacts
        .iter()
        .map(|artifact| artifact["path"].as_str().unwrap())
        .collect();
    assert_eq!(
        orchestration_paths,
        vec![
            "tool-artifacts/ingest_tool_claims",
            "tool-artifacts/manifest-ghidra-distribution.py",
            "tool-artifacts/memory-guard.zsh",
            "tool-artifacts/run-snapshot-bank.sh",
            "tool-artifacts/stage_snapshot_bank",
        ]
    );
    let orchestration_sha = |path: &str| {
        orchestration_artifacts
            .iter()
            .find(|artifact| artifact["path"] == path)
            .unwrap()["sha256"]
            .as_str()
            .unwrap()
    };
    assert_eq!(
        orchestration_sha("tool-artifacts/ingest_tool_claims"),
        Sha256Digest::of(&fs::read(&ingest).unwrap()).to_hex()
    );
    assert_eq!(
        orchestration_sha("tool-artifacts/run-snapshot-bank.sh"),
        Sha256Digest::of(&fs::read(repo.join("tools/ghidra/run-snapshot-bank.sh")).unwrap())
            .to_hex()
    );

    make_executable(
        &support.join("analyzeHeadless"),
        r#"#!/bin/sh
set -eu
provider=
for argument in "$@"; do
    case "$argument" in
        Fn64SeedFunctions.java) exit 91 ;;
        */provider.jsonl) provider=$argument ;;
    esac
done
[ -n "$provider" ]
printf '{}\n' > "$provider"
echo 'Using Loader: Raw Binary'
echo 'Using Language/Compiler: MIPS:BE:64:64-32addr:o32'
"#,
    );
    let discovery = Command::new(repo.join("tools/ghidra/run-snapshot-bank.sh"))
        .arg("--discovery-only")
        .arg(&fixture.snapshot)
        .arg("boot")
        .arg(&fixture.bank)
        .arg(&fixture.workspace)
        .env(
            "FN64_STAGE_SNAPSHOT_BANK",
            env!("CARGO_BIN_EXE_stage_snapshot_bank"),
        )
        .env("FN64_INGEST_TOOL_CLAIMS", &ingest)
        .env("GHIDRA_INSTALL_DIR", &ghidra)
        .env("GHIDRA_JAVA_HOME", &jdk)
        .output()
        .unwrap();
    assert!(
        discovery.status.success(),
        "stdout:\n{}\nstderr:\n{}\ndiagnostics:{}",
        String::from_utf8_lossy(&discovery.stdout),
        String::from_utf8_lossy(&discovery.stderr),
        retained_diagnostics(&fixture.workspace)
    );
    let discovery_attempt = String::from_utf8(discovery.stdout)
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("attempt="))
        .map(PathBuf::from)
        .expect("discovery-only runner did not print its attempt path");
    let discovery_evidence: serde_json::Value =
        serde_json::from_slice(&fs::read(discovery_attempt.join("raw/evidence.json")).unwrap())
            .unwrap();
    assert_eq!(discovery_evidence["schema_version"], 3);
    assert_eq!(discovery_evidence["seeds"]["mode"], "discovery_only");
    assert_eq!(discovery_evidence["seeds"]["role"], "candidate_only");
    assert!(discovery_evidence["seeds"].get("base_seed").is_none());
    assert!(!discovery_attempt
        .join("tool-artifacts/Fn64SeedFunctions.java")
        .exists());
    let discovery_config: serde_json::Value =
        serde_json::from_slice(&fs::read(discovery_attempt.join("config/unseeded.json")).unwrap())
            .unwrap();
    assert_eq!(discovery_config["role"], "candidate_only");
    assert_eq!(discovery_config["base_seed"], serde_json::Value::Null);
    assert_eq!(discovery_config["snapshot_seed"], serde_json::Value::Null);
    let discovery_request: serde_json::Value =
        serde_json::from_slice(&fs::read(discovery_attempt.join("request.json")).unwrap()).unwrap();
    assert_eq!(
        discovery_request["runs"][0]["role"],
        "function_boundary_candidates"
    );
    let discovery_receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(discovery_attempt.join("out/receipt.json")).unwrap())
            .unwrap();
    assert_eq!(discovery_receipt["execution_mode"], "discovery-only");
    assert_eq!(discovery_receipt["seeds"]["mode"], "discovery_only");
    assert_eq!(discovery_receipt["seeds"]["role"], "candidate_only");

    make_executable(&support.join("analyzeHeadless"), headless_script);

    let paired = Command::new(repo.join("tools/ghidra/run-snapshot-bank.sh"))
        .arg(&fixture.snapshot)
        .arg("boot")
        .arg(&fixture.bank)
        .arg(&fixture.workspace)
        .arg(format!("0x{BASE:08x}"))
        .arg(format!("0x{EXTRA:08x}"))
        .env(
            "FN64_STAGE_SNAPSHOT_BANK",
            env!("CARGO_BIN_EXE_stage_snapshot_bank"),
        )
        .env("FN64_INGEST_TOOL_CLAIMS", &ingest)
        .env("GHIDRA_INSTALL_DIR", &ghidra)
        .env("GHIDRA_JAVA_HOME", &jdk)
        .output()
        .unwrap();
    assert!(
        paired.status.success(),
        "stdout:\n{}\nstderr:\n{}\ndiagnostics:{}",
        String::from_utf8_lossy(&paired.stdout),
        String::from_utf8_lossy(&paired.stderr),
        retained_diagnostics(&fixture.workspace)
    );
    let paired_stdout = String::from_utf8(paired.stdout).unwrap();
    let paired_attempt = paired_stdout
        .lines()
        .find_map(|line| line.strip_prefix("attempt="))
        .map(PathBuf::from)
        .expect("paired runner did not print its attempt path");
    let paired_evidence: serde_json::Value =
        serde_json::from_slice(&fs::read(paired_attempt.join("raw/evidence.json")).unwrap())
            .unwrap();
    assert_eq!(paired_evidence["seeds"]["mode"], "paired");
    assert_eq!(paired_evidence["seeds"]["base_seed"], BASE);
    assert_eq!(paired_evidence["seeds"]["snapshot_seed"], EXTRA);
    let paired_receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(paired_attempt.join("out/receipt.json")).unwrap())
            .unwrap();
    assert_eq!(paired_receipt["execution_mode"], "paired");
    assert_eq!(paired_receipt["paired_comparison_complete"], true);
    assert_eq!(paired_receipt["seeds"]["mode"], "paired");
    assert_eq!(paired_receipt["seeds"]["snapshot_seed"], EXTRA);
    assert!(paired_attempt.join("config/seeded.json").is_file());
    assert!(paired_attempt.join("tool-seeded.json").is_file());
    assert!(paired_attempt
        .join("modes/seeded/out/provider.jsonl")
        .is_file());

    let escaped_ingest = ingest.to_string_lossy().replace('\'', "'\"'\"'");
    let mutating_headless = format!(
        r#"#!/bin/sh
set -eu
printf 'mutated\n' > '{escaped_ingest}'
provider=
for argument in "$@"; do
    case "$argument" in
        */provider.jsonl) provider=$argument ;;
    esac
done
[ -n "$provider" ]
printf '{{}}\n' > "$provider"
echo 'Using Loader: Raw Binary'
echo 'Using Language/Compiler: MIPS:BE:64:64-32addr:o32'
"#
    );
    make_executable(&support.join("analyzeHeadless"), &mutating_headless);
    let mutated = Command::new(repo.join("tools/ghidra/run-snapshot-bank.sh"))
        .arg("--unseeded-only")
        .arg(&fixture.snapshot)
        .arg("boot")
        .arg(&fixture.bank)
        .arg(&fixture.workspace)
        .arg(format!("0x{BASE:08x}"))
        .env(
            "FN64_STAGE_SNAPSHOT_BANK",
            env!("CARGO_BIN_EXE_stage_snapshot_bank"),
        )
        .env("FN64_INGEST_TOOL_CLAIMS", &ingest)
        .env("GHIDRA_INSTALL_DIR", &ghidra)
        .env("GHIDRA_JAVA_HOME", &jdk)
        .output()
        .unwrap();
    assert!(!mutated.status.success());
    assert!(
        String::from_utf8_lossy(&mutated.stderr)
            .contains("ingest helper source changed during run"),
        "stdout:\n{}\nstderr:\n{}\ndiagnostics:{}",
        String::from_utf8_lossy(&mutated.stdout),
        String::from_utf8_lossy(&mutated.stderr),
        retained_diagnostics(&fixture.workspace)
    );
}
