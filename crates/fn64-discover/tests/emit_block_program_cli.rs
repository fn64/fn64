use fn64_discover::block_pack::{BlockPackV1, PackedBankV1, PackedBlockV1, BLOCK_PACK_SCHEMA_V1};
use fn64_discover::cfg::BlockTerminator;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

const BANK_A: u64 = 0xAABB_CCDD_EEFF_0011;
const BANK_B: u64 = 0xBBCC_DDEE_FF00_1122;
const ENTRY_PC: u32 = 0x8000_1000;
const HOLE_PC: u32 = 0x8000_1010;
const ROM_BASE: u32 = 0x1000;

struct Fixture {
    root: PathBuf,
    rom: PathBuf,
    pack: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = (0..128u32)
            .find_map(|attempt| {
                let candidate = std::env::temp_dir().join(format!(
                    "fn64-emit-block-program-cli-{}-{nonce}-{attempt}",
                    std::process::id()
                ));
                match std::fs::create_dir(&candidate) {
                    Ok(()) => Some(candidate),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => None,
                    Err(error) => panic!(
                        "create synthetic CLI fixture directory {}: {error}",
                        candidate.display()
                    ),
                }
            })
            .expect("could not reserve a synthetic CLI fixture directory after 128 attempts");
        let rom = root.join("synthetic.z64");
        let pack = root.join("block-pack.json");
        let (rom_bytes, block_pack) = synthetic_pack();
        std::fs::write(&rom, rom_bytes).unwrap();
        std::fs::write(&pack, serde_json::to_vec_pretty(&block_pack).unwrap()).unwrap();
        Self { root, rom, pack }
    }

    fn output(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn cli_emits_deterministic_source_and_content_receipt_with_overlap_retained() {
    let fixture = Fixture::new();
    let first_path = fixture.output("first.rs");
    let second_path = fixture.output("second.rs");
    let first = run_emit(&fixture.rom, &fixture.pack, &first_path, ENTRY_PC);
    let second = run_emit(&fixture.rom, &fixture.pack, &second_path, ENTRY_PC);
    assert_success(&first);
    assert_success(&second);

    let first_bytes = std::fs::read(&first_path).unwrap();
    let second_bytes = std::fs::read(&second_path).unwrap();
    assert_eq!(first_bytes, second_bytes);
    let digest = hex(Sha256::digest(&first_bytes).into());
    assert_eq!(
        String::from_utf8(first.stdout).unwrap(),
        format!(
            "fn64-discover emit-block-program: sha256={digest} bytes={} out={}\n",
            first_bytes.len(),
            first_path.display()
        )
    );
    let source = String::from_utf8(first_bytes).unwrap();
    assert!(source.contains("pub fn build_block_program("));
    assert!(source.contains("pub fn entry_lookup("));
    assert!(source.contains("CpuFaultKind::AmbiguousPc"));
    assert!(source.contains("GeneratedBankRunner::new_with_artifact_identity"));
    assert!(!source.contains("GeneratedBankRunner::new("));
}

#[test]
fn cli_rejects_wrong_rom_schema_entry_and_unknown_pack_fields() {
    let fixture = Fixture::new();

    let mut wrong_rom = std::fs::read(&fixture.rom).unwrap();
    wrong_rom[0x40] ^= 1;
    let wrong_rom_path = fixture.output("wrong-rom.z64");
    std::fs::write(&wrong_rom_path, wrong_rom).unwrap();
    let wrong_rom_output = run_emit(
        &wrong_rom_path,
        &fixture.pack,
        &fixture.output("wrong-rom.rs"),
        ENTRY_PC,
    );
    assert_failure_contains(&wrong_rom_output, "RomIdentityMismatch");

    let mut schema: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&fixture.pack).unwrap()).unwrap();
    schema["schema_version"] = serde_json::json!(BLOCK_PACK_SCHEMA_V1 + 1);
    let schema_path = fixture.output("wrong-schema.json");
    std::fs::write(&schema_path, serde_json::to_vec(&schema).unwrap()).unwrap();
    let schema_output = run_emit(
        &fixture.rom,
        &schema_path,
        &fixture.output("wrong-schema.rs"),
        ENTRY_PC,
    );
    assert_failure_contains(&schema_output, "UnsupportedSchema");

    let entry_output = run_emit(
        &fixture.rom,
        &fixture.pack,
        &fixture.output("wrong-entry.rs"),
        HOLE_PC,
    );
    assert_failure_contains(
        &entry_output,
        "declared block-program entry is not admitted",
    );

    let mut unknown: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&fixture.pack).unwrap()).unwrap();
    unknown["banks"][0]["guessed_entry"] = serde_json::json!(ENTRY_PC);
    let unknown_path = fixture.output("unknown-field.json");
    std::fs::write(&unknown_path, serde_json::to_vec(&unknown).unwrap()).unwrap();
    let unknown_output = run_emit(
        &fixture.rom,
        &unknown_path,
        &fixture.output("unknown.rs"),
        ENTRY_PC,
    );
    assert_failure_contains(&unknown_output, "unknown field `guessed_entry`");
}

#[test]
fn cli_rejects_noncanonical_numeric_inputs_and_requires_safe_explicit_output() {
    let fixture = Fixture::new();
    let binary = env!("CARGO_BIN_EXE_fn64-discover");

    let lowercase = Command::new(binary)
        .args([
            "emit-block-program",
            fixture.rom.to_str().unwrap(),
            fixture.pack.to_str().unwrap(),
            "--entry-bank",
            "0xAABBCCDDEEFF00aa",
            "--entry-pc",
            "0x80001000",
            "--instruction-budget",
            "2",
            "--out",
            fixture.output("lowercase.rs").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_failure_contains(&lowercase, "must use uppercase hexadecimal digits");

    let malformed_pc = Command::new(binary)
        .args([
            "emit-block-program",
            fixture.rom.to_str().unwrap(),
            fixture.pack.to_str().unwrap(),
            "--entry-bank",
            "0xAABBCCDDEEFF0011",
            "--entry-pc",
            "0x8000100G",
            "--instruction-budget",
            "2",
            "--out",
            fixture.output("malformed-pc.rs").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_failure_contains(&malformed_pc, "contains a non-hexadecimal digit");

    let overflow = Command::new(binary)
        .args([
            "emit-block-program",
            fixture.rom.to_str().unwrap(),
            fixture.pack.to_str().unwrap(),
            "--entry-bank",
            "0x0AABBCCDDEEFF0011",
            "--entry-pc",
            "0x80001000",
            "--instruction-budget",
            "4294967296",
            "--out",
            fixture.output("overflow.rs").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_failure_contains(&overflow, "exactly 16 hexadecimal digits");

    let budget_overflow = Command::new(binary)
        .args([
            "emit-block-program",
            fixture.rom.to_str().unwrap(),
            fixture.pack.to_str().unwrap(),
            "--entry-bank",
            "0xAABBCCDDEEFF0011",
            "--entry-pc",
            "0x80001000",
            "--instruction-budget",
            "4294967296",
            "--out",
            fixture.output("budget-overflow.rs").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_failure_contains(&budget_overflow, "exceeds u32 or is malformed");

    let leading_zero_budget = Command::new(binary)
        .args([
            "emit-block-program",
            fixture.rom.to_str().unwrap(),
            fixture.pack.to_str().unwrap(),
            "--entry-bank",
            "0xAABBCCDDEEFF0011",
            "--entry-pc",
            "0x80001000",
            "--instruction-budget",
            "0002",
            "--out",
            fixture.output("leading-zero-budget.rs").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_failure_contains(&leading_zero_budget, "must not contain leading zeros");

    let no_output = Command::new(binary)
        .args([
            "emit-block-program",
            fixture.rom.to_str().unwrap(),
            fixture.pack.to_str().unwrap(),
            "--entry-bank",
            "0xAABBCCDDEEFF0011",
            "--entry-pc",
            "0x80001000",
            "--instruction-budget",
            "2",
        ])
        .output()
        .unwrap();
    assert_failure_contains(&no_output, "requires explicit --out");

    let output_directory = fixture.output("directory.rs");
    std::fs::create_dir(&output_directory).unwrap();
    let output_failure = run_emit(&fixture.rom, &fixture.pack, &output_directory, ENTRY_PC);
    assert_failure_contains(
        &output_failure,
        "publishing generated source without clobber",
    );
    assert!(output_directory.is_dir());

    let existing_output = fixture.output("existing.rs");
    std::fs::write(&existing_output, b"do not replace\n").unwrap();
    let existing_failure = run_emit(&fixture.rom, &fixture.pack, &existing_output, ENTRY_PC);
    assert_failure_contains(
        &existing_failure,
        "publishing generated source without clobber",
    );
    assert_eq!(
        std::fs::read(&existing_output).unwrap(),
        b"do not replace\n"
    );
    assert!(std::fs::read_dir(&fixture.root)
        .unwrap()
        .filter_map(Result::ok)
        .all(|entry| !entry.file_name().to_string_lossy().contains("fn64-tmp")));
}

#[test]
fn legacy_discovery_invocation_remains_available_without_a_subcommand() {
    let fixture = Fixture::new();
    let output_path = fixture.output("discovery.json");
    let output = Command::new(env!("CARGO_BIN_EXE_fn64-discover"))
        .args([
            fixture.rom.to_str().unwrap(),
            "--out",
            output_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_success(&output);
    let artifact: serde_json::Value =
        serde_json::from_slice(&std::fs::read(output_path).unwrap()).unwrap();
    assert_eq!(artifact["schema_version"], 1);
    assert_eq!(artifact["rom"]["sha256"].as_str().unwrap().len(), 64);
}

fn run_emit(rom: &Path, pack: &Path, out: &Path, entry_pc: u32) -> Output {
    Command::new(env!("CARGO_BIN_EXE_fn64-discover"))
        .args([
            "emit-block-program",
            rom.to_str().unwrap(),
            pack.to_str().unwrap(),
            "--entry-bank",
            "0xAABBCCDDEEFF0011",
            "--entry-pc",
            &format!("{entry_pc:#010X}"),
            "--instruction-budget",
            "2",
            "--out",
            out.to_str().unwrap(),
        ])
        .output()
        .unwrap()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "command failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_failure_contains(output: &Output, needle: &str) {
    assert!(!output.status.success(), "command unexpectedly succeeded");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(needle),
        "stderr did not contain {needle:?}:\n{stderr}"
    );
}

fn synthetic_pack() -> (Vec<u8>, BlockPackV1) {
    let words = [0x2402_0007u32, 0, 0x2402_0009, 0x2403_0005];
    let mut bytes = vec![0u8; ROM_BASE as usize + words.len() * 4];
    bytes[..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
    bytes[8..12].copy_from_slice(&ENTRY_PC.to_be_bytes());
    for (index, word) in words.into_iter().enumerate() {
        let start = ROM_BASE as usize + index * 4;
        bytes[start..start + 4].copy_from_slice(&word.to_be_bytes());
    }
    let rom = fn64_discover::normalize(&bytes).unwrap();
    let block = |start_va: u32, word_index: u32| {
        let rom_start = ROM_BASE + word_index * 4;
        PackedBlockV1 {
            start_va,
            end_va: start_va + 4,
            rom_start,
            rom_end: rom_start + 4,
            bytes_sha256: hex(Sha256::digest(
                &rom.bytes[rom_start as usize..rom_start as usize + 4],
            )
            .into()),
            terminator: BlockTerminator::Fallthrough { next: start_va + 4 },
        }
    };
    let pack = BlockPackV1 {
        schema_version: BLOCK_PACK_SCHEMA_V1,
        normalized_rom_sha256: rom.sha256,
        banks: vec![
            PackedBankV1 {
                bank: "resident".into(),
                bank_id: BANK_A,
                blocks: vec![block(ENTRY_PC, 0), block(0x8000_1020, 1)],
            },
            PackedBankV1 {
                bank: "overlay".into(),
                bank_id: BANK_B,
                blocks: vec![block(ENTRY_PC, 2), block(0x8000_2000, 3)],
            },
        ],
    };
    (bytes, pack)
}

fn hex(bytes: [u8; 32]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
