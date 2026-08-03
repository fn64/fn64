//! Public synthetic coverage for the compact discovery feedback command.
//!
//! The summary receipt is deliberately not a discovery artifact: this test
//! proves its deterministic observation shape and rejects combinations that
//! could otherwise make callers mistake it for an artifact write.

use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

struct Fixture {
    root: PathBuf,
    rom: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time after Unix epoch")
            .as_nanos();
        let root = (0..128u32)
            .find_map(|attempt| {
                let candidate = std::env::temp_dir().join(format!(
                    "fn64-discovery-summary-cli-{}-{nonce}-{attempt}",
                    std::process::id()
                ));
                match std::fs::create_dir(&candidate) {
                    Ok(()) => Some(candidate),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => None,
                    Err(error) => panic!(
                        "create synthetic summary fixture directory {}: {error}",
                        candidate.display()
                    ),
                }
            })
            .expect("could not reserve synthetic summary fixture directory after 128 attempts");
        let rom = root.join("synthetic.z64");
        std::fs::write(&rom, synthetic_rom()).expect("write synthetic ROM");
        Self { root, rom }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).expect("remove synthetic summary fixture");
    }
}

fn synthetic_rom() -> Vec<u8> {
    let mut bytes = vec![0u8; 0x3000];
    bytes[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
    bytes[8..12].copy_from_slice(&0x8000_0400u32.to_be_bytes());
    bytes[0x20..0x24].copy_from_slice(b"TEST");
    bytes[0x3b..0x3f].copy_from_slice(b"CTSE");
    bytes[0x100..0x104].copy_from_slice(&0x1000u32.to_be_bytes());
    bytes[0x104..0x108].copy_from_slice(&0x1010u32.to_be_bytes());
    bytes[0x108..0x10c].copy_from_slice(&0x8000_0400u32.to_be_bytes());
    bytes[0x1000..0x1004].copy_from_slice(&0x03e0_0008u32.to_be_bytes());
    bytes
}

fn discover(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_fn64-discover"))
        .args(args)
        .output()
        .expect("run fn64-discover")
}

#[test]
fn summary_is_deterministic_compact_and_path_free() {
    let fixture = Fixture::new();
    let rom = fixture.rom.to_str().expect("UTF-8 temporary path");
    let first = discover(&[rom, "--summary"]);
    let second = discover(&[rom, "--summary"]);
    assert!(first.status.success(), "first summary failed: {first:?}");
    assert!(second.status.success(), "second summary failed: {second:?}");
    assert_eq!(
        first.stdout, second.stdout,
        "summary receipt must be stable"
    );

    let stdout = String::from_utf8(first.stdout).expect("summary is UTF-8 JSON");
    assert!(
        !stdout.contains(fixture.root.to_str().expect("UTF-8 temporary path")),
        "summary must not retain the input path"
    );
    let receipt: serde_json::Value = serde_json::from_str(&stdout).expect("summary JSON");
    let artifact_run = discover(&[rom]);
    assert!(
        artifact_run.status.success(),
        "full artifact failed: {artifact_run:?}"
    );
    let artifact: serde_json::Value =
        serde_json::from_slice(&artifact_run.stdout).expect("full artifact JSON");
    assert_eq!(receipt["summary"]["schema_version"], 1);
    assert!(receipt["summary"].get("owner_proof").is_none());
    assert!(receipt["summary"]["normalized_rom_sha256"]
        .as_str()
        .is_some_and(|digest| digest.len() == 64));
    assert_eq!(receipt["summary"]["selected_strategy"], "boot_bank_open");
    assert_eq!(
        receipt["summary"]["selected_strategy"], artifact["selected_strategy"],
        "summary and full artifact must report the same selected strategy"
    );
    assert_eq!(
        receipt["summary"]["coverage"], artifact["coverage"],
        "summary and full artifact must report identical coverage"
    );
    assert_eq!(
        receipt["summary"]["fact_count"].as_u64(),
        artifact["facts"]["facts"]
            .as_array()
            .map(|facts| facts.len() as u64),
        "summary fact count must equal the full artifact fact log"
    );
    assert_eq!(receipt["summary"]["trace_count"], 0);
    assert_eq!(receipt["summary"]["observed_load_image_reports"], 0);
    assert!(receipt["receipt_sha256"]
        .as_str()
        .is_some_and(|digest| digest.len() == 64));
}

#[test]
fn summary_rejects_artifact_output_combination() {
    let fixture = Fixture::new();
    let rom = fixture.rom.to_str().expect("UTF-8 temporary path");
    let output = fixture.root.join("must-not-exist.json");
    let output = output.to_str().expect("UTF-8 temporary path");
    let result = discover(&[rom, "--summary", "--out", output]);
    assert!(!result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stderr)
            .contains("--summary and --out are mutually exclusive"),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        !std::path::Path::new(output).exists(),
        "rejected argument shape must not write an artifact"
    );
}

#[test]
fn owner_summary_retains_composed_executable_evidence_and_frontiers() {
    let fixture = Fixture::new();
    let rom = fixture.rom.to_str().expect("UTF-8 temporary path");
    let identity = discover(&[rom, "--summary"]);
    assert!(
        identity.status.success(),
        "identity summary failed: {identity:?}"
    );
    let identity: serde_json::Value =
        serde_json::from_slice(&identity.stdout).expect("identity summary JSON");
    let digest = identity["summary"]["normalized_rom_sha256"]
        .as_str()
        .expect("normalized digest");
    let evidence = fixture.root.join("evidence.toml");
    std::fs::write(
        &evidence,
        format!(
            r#"schema_version = 1
rom_sha256 = "{digest}"

[[descriptor_tables]]
name = "synthetic_boot_table"
source = "synthetic summary test"

[descriptor_tables.shape]
table_rom_offset = 256
record_count = 1
record_stride = 12
field_rom_start = 0
field_rom_end = 4
field_vram_dest = 8

[descriptor_tables.bank_name]
prefix = "boot"
suffix = ""
index_base = 0

[[executable_ranges]]
bank = "boot0"
va_start = 2147484672
va_end = 2147484688
source = "synthetic summary test text"
"#,
        ),
    )
    .expect("write evidence manifest");
    let evidence = evidence.to_str().expect("UTF-8 evidence path");
    let result = discover(&[rom, "--evidence", evidence, "--summary", "--prove-owners"]);
    assert!(result.status.success(), "owner summary failed: {result:?}");
    let receipt: serde_json::Value =
        serde_json::from_slice(&result.stdout).expect("owner summary JSON");
    let summary = &receipt["summary"];
    assert_eq!(summary["schema_version"], 2);
    assert_eq!(
        summary["coverage"]["function_owners"]["blockers"],
        serde_json::json!([]),
        "compact summary must not retain ROM-bearing blocker payloads"
    );
    assert!(
        result.stdout.len() < 256 * 1024,
        "synthetic compact owner receipt unexpectedly grew to {} bytes",
        result.stdout.len()
    );
    let owner = &summary["owner_proof"];
    assert_eq!(owner["coverage_blocker_payloads_omitted"], true);
    let owner_json = serde_json::to_string(owner).expect("serialize owner diagnostic");
    for forbidden in [
        "\"word\"",
        "\"site_pc\"",
        "\"targets\"",
        "\"memory_sources\"",
    ] {
        assert!(
            !owner_json.contains(forbidden),
            "content-free owner diagnostic retained {forbidden}"
        );
    }
    let banks = owner["banks"].as_array().unwrap_or_else(|| {
        panic!(
            "per-bank owner diagnostics; stderr={}",
            String::from_utf8_lossy(&result.stderr)
        )
    });
    assert!(!banks.is_empty());
    assert_eq!(
        banks
            .iter()
            .map(|bank| bank["assessed_entries"].as_u64().unwrap())
            .sum::<u64>(),
        summary["coverage"]["function_owners"]["assessed_entries"]
            .as_u64()
            .unwrap()
    );
    assert_eq!(
        owner["executable_ranges"]["proven_bytes"], summary["coverage"]["executable_bytes"],
        "ROM-wide coverage must retain executable ranges derived inside composed banks"
    );
    assert!(
        owner["executable_ranges"]["proven_range_count"]
            .as_u64()
            .is_some_and(|count| count > 0),
        "authoritative synthetic entry should derive executable evidence"
    );
    for marginal in owner["blocker_marginals"]
        .as_array()
        .expect("aggregate blocker marginals")
    {
        assert!(marginal["kind"].as_str().is_some());
        assert!(marginal["sole_blocker_assessments"].as_u64().is_some());
    }
    for combination in owner["blocker_combinations"]
        .as_array()
        .expect("aggregate blocker combinations")
    {
        assert!(matches!(
            combination["assessment_state"].as_str(),
            Some("candidate" | "ambiguous")
        ));
        assert!(combination["kinds"].as_array().is_some());
    }
    let indirect = &owner["indirect_transfers"];
    assert_eq!(
        indirect["total_sites"].as_u64().unwrap(),
        indirect["exhaustive_sites"].as_u64().unwrap()
            + indirect["bounded_sites"].as_u64().unwrap()
            + indirect["open_sites"].as_u64().unwrap()
    );
    assert_eq!(
        indirect["total_sites"].as_u64().unwrap(),
        indirect["via_call_sites"].as_u64().unwrap() + indirect["via_jump_sites"].as_u64().unwrap()
    );
}
