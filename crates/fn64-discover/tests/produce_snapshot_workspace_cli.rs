use fn64_discover::grade_candidates::ScopedCandidateIdentitiesV3;
use fn64_discover::snapshot::ProgramSnapshotV1;
use fn64_discover::tool_adapter::Sha256Digest;
use fn64_discover::tool_claims::program_snapshot_sha256_v3;
use std::collections::BTreeSet;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

struct Fixture {
    root: PathBuf,
    workspace: PathBuf,
    rom: PathBuf,
}

impl Fixture {
    fn new(label: &str, rom_bytes: &[u8]) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "fn64-produce-snapshot-workspace-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let workspace = root.join("workspace");
        fs::create_dir(&workspace).unwrap();
        #[cfg(unix)]
        fs::set_permissions(&workspace, fs::Permissions::from_mode(0o700)).unwrap();
        let workspace = fs::canonicalize(workspace).unwrap();
        let rom = root.join("input.z64");
        fs::write(&rom, rom_bytes).unwrap();
        Self {
            root,
            workspace,
            rom,
        }
    }

    fn run(&self) -> Output {
        run(&self.rom, &self.workspace)
    }

    fn run_selected(&self, bank: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_produce_snapshot_workspace"))
            .args(["--bank", bank])
            .arg(&self.rom)
            .arg(&self.workspace)
            .output()
            .unwrap()
    }

    fn run_training(&self) -> Output {
        Command::new(env!("CARGO_BIN_EXE_produce_snapshot_workspace"))
            .arg("--training")
            .arg(&self.rom)
            .arg(&self.workspace)
            .output()
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

fn run(rom: &Path, workspace: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_produce_snapshot_workspace"))
        .arg(rom)
        .arg(workspace)
        .output()
        .unwrap()
}

fn open_rom() -> Vec<u8> {
    let mut bytes = vec![0u8; 0x3000];
    put_u32(&mut bytes, 0, 0x8037_1240);
    put_u32(&mut bytes, 8, 0x8000_0400);
    bytes[0x20..0x24].copy_from_slice(b"TEST");
    bytes[0x3b..0x3f].copy_from_slice(b"CTSE");
    bytes
}

fn recovered_vrom_rom() -> Vec<u8> {
    let mut bytes = vec![0u8; 0xe000];
    put_u32(&mut bytes, 0, 0x8037_1240);
    put_u32(&mut bytes, 8, 0x8000_0400);
    for (index, fields) in [
        [0x0000, 0x3000, 0x0000, 0x0000],
        [0x3000, 0x6000, 0x8000, 0x0000],
        [0x6000, 0x9000, 0xb000, 0x0000],
    ]
    .into_iter()
    .enumerate()
    {
        for (field, value) in fields.into_iter().enumerate() {
            put_u32(&mut bytes, 0x2000 + index * 0x10 + field * 4, value);
        }
    }
    for (index, (vrom_start, vrom_end, vram)) in
        [(0x6000, 0x6800, 0x8002_0000), (0x7000, 0x7800, 0x8003_0000)]
            .into_iter()
            .enumerate()
    {
        let base = 0x8000 + index * 0x1c;
        put_u32(&mut bytes, base, vrom_start);
        put_u32(&mut bytes, base + 4, vrom_end);
        put_u32(&mut bytes, base + 8, vram);
        let physical = 0xb000 + (vrom_start - 0x6000) as usize;
        plant_delta_admissible_region(&mut bytes, physical, vram);
    }
    bytes
}

fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}

fn plant_delta_admissible_region(bytes: &mut [u8], physical_start: usize, va_start: u32) {
    let jal = |target: u32| 0x0c00_0000 | ((target >> 2) & 0x03ff_ffff);
    for (offset, target_offset) in [(0, 0x40), (8, 0x90), (16, 0x100)] {
        put_u32(
            bytes,
            physical_start + offset,
            jal(va_start + target_offset),
        );
    }
    for offset in [0x40, 0x90, 0x100] {
        put_u32(bytes, physical_start + offset, 0x27bd_ffe0);
    }
    for offset in [0x20, 0x24, 0x28, 0x2c] {
        put_u32(
            bytes,
            physical_start + offset,
            0x3c04_0000 | (va_start >> 16),
        );
    }
}

#[test]
fn zero_proven_banks_publish_only_an_honest_path_free_open_manifest() {
    let fixture = Fixture::new("open", &open_rom());
    let output = fixture.run();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let names: Vec<_> = fs::read_dir(&fixture.workspace)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names, ["snapshot-workspace.json"]);
    let manifest_bytes = fs::read(fixture.workspace.join("snapshot-workspace.json")).unwrap();
    let manifest: serde_json::Value = serde_json::from_slice(&manifest_bytes).unwrap();
    assert_eq!(manifest["schema"], "fn64.snapshot-workspace");
    assert_eq!(manifest["state"], "open");
    assert_eq!(manifest["open_reason"], "no_proven_banks");
    assert_eq!(manifest["banks"].as_array().unwrap().len(), 0);
    assert_eq!(manifest["discovery"]["selected"], "boot_bank_open");
    assert!(manifest["discovery"]["outcomes"]
        .as_array()
        .unwrap()
        .iter()
        .all(|outcome| outcome.get("decoded_file_limit_hits").is_some()));
    assert_eq!(manifest["snapshot_wire"]["schema_version"], 6);
    assert_eq!(manifest["snapshot_wire"]["authority"], "diagnostic_only");
    assert_eq!(
        manifest["snapshot_wire"]["duplicates_fact_db_per_bank"],
        false
    );
    assert_eq!(
        manifest["snapshot_wire"]["remaining_large_rom_frontier"],
        "streaming_v6"
    );
    assert_eq!(manifest["aggregate_snapshot_artifact_bytes"], 0);
    assert_eq!(manifest["rom_recompilation_complete"], false);
    assert_eq!(manifest["intended_use"], "candidate_ghidra_only");
    assert_eq!(manifest["limits"]["max_rom_bytes"], 64 * 1024 * 1024u64);
    assert_eq!(
        manifest["limits"]["max_discovery_decoded_vrom_file_bytes"],
        64 * 1024 * 1024u64
    );
    assert!(!String::from_utf8(manifest_bytes)
        .unwrap()
        .contains(fixture.root.to_str().unwrap()));

    let repeated = fixture.run();
    assert!(!repeated.status.success());
    assert!(String::from_utf8_lossy(&repeated.stderr).contains("refusing to overwrite"));
}

#[test]
fn fresh_workspaces_produce_byte_identical_open_manifests() {
    let first = Fixture::new("determinism-a", &open_rom());
    let second = Fixture::new("determinism-b", &open_rom());
    assert!(first.run().status.success());
    assert!(second.run().status.success());
    assert_eq!(
        fs::read(first.workspace.join("snapshot-workspace.json")).unwrap(),
        fs::read(second.workspace.join("snapshot-workspace.json")).unwrap()
    );
}

#[test]
fn training_mode_seals_key_free_candidate_receipt_even_when_no_bank_is_proven() {
    let fixture = Fixture::new("training-open", &open_rom());
    let output = fixture.run_training();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let manifest_bytes = fs::read(fixture.workspace.join("snapshot-workspace.json")).unwrap();
    let manifest: serde_json::Value = serde_json::from_slice(&manifest_bytes).unwrap();
    assert_eq!(manifest["schema_version"], 4);
    assert_eq!(manifest["state"], "open");
    assert_eq!(
        manifest["intended_use"],
        "sealed_cold_function_training_input"
    );
    assert_eq!(manifest["cold_training"]["schema_version"], 3);
    assert_eq!(
        manifest["cold_training"]["algorithm"],
        "fn64.cold-function-training.v3"
    );
    assert_eq!(manifest["cold_training"]["answer_key_present"], false);
    assert_eq!(
        manifest["cold_training"]["candidate_artifact"],
        "cold-candidates.json"
    );
    let candidate_bytes = fs::read(fixture.workspace.join("cold-candidates.json")).unwrap();
    let identities: ScopedCandidateIdentitiesV3 = serde_json::from_slice(&candidate_bytes).unwrap();
    assert_eq!(
        manifest["cold_training"]["candidate_artifact_byte_length"],
        candidate_bytes.len()
    );
    assert_eq!(
        manifest["cold_training"]["candidate_artifact_sha256"],
        Sha256Digest::of(&candidate_bytes).to_hex()
    );
    assert_eq!(
        manifest["cold_training"]["scoped_candidate_identities_v3_sha256"],
        identities.digest_sha256()
    );
    assert_eq!(fs::read_dir(&fixture.workspace).unwrap().count(), 2);
    assert!(!String::from_utf8(manifest_bytes)
        .unwrap()
        .contains(fixture.root.to_str().unwrap()));
}

#[test]
fn dirty_reserved_namespace_rejects_even_an_open_result() {
    let fixture = Fixture::new("dirty-open", &open_rom());
    fs::write(
        fixture.workspace.join("bank-999999.snapshot.json"),
        b"winner",
    )
    .unwrap();
    let output = fixture.run();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("reserved snapshot artifact"));
    assert!(!fixture.workspace.join("snapshot-workspace.json").exists());
}

#[test]
fn oversized_rom_cap_failure_leaves_no_manifest() {
    let fixture = Fixture::new("rom-cap", &open_rom());
    fs::OpenOptions::new()
        .write(true)
        .open(&fixture.rom)
        .unwrap()
        .set_len(64 * 1024 * 1024 + 1)
        .unwrap();
    let output = fixture.run();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("ROM exceeds"));
    assert!(!fixture.workspace.join("snapshot-workspace.json").exists());
}

#[test]
fn recovered_banks_use_fixed_index_names_and_manifest_bound_digests() {
    let fixture = Fixture::new("recovered", &recovered_vrom_rom());
    let output = fixture.run();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let manifest_bytes = fs::read(fixture.workspace.join("snapshot-workspace.json")).unwrap();
    let manifest: serde_json::Value = serde_json::from_slice(&manifest_bytes).unwrap();
    assert_eq!(manifest["state"], "composed");
    assert_eq!(manifest["open_reason"], serde_json::Value::Null);
    assert_eq!(manifest["rom_recompilation_complete"], false);
    assert_eq!(manifest["discovery"]["selected"], "recovered_vrom");
    let banks = manifest["banks"].as_array().unwrap();
    assert_eq!(banks.len(), 2);
    let mut aggregate = 0u64;
    let mut prior_bank_name: Option<&str> = None;
    let mut unique_bank_names = BTreeSet::new();
    for (index, bank) in banks.iter().enumerate() {
        let bin_name = format!("bank-{index:06}.bin");
        let snapshot_name = format!("bank-{index:06}.snapshot.json");
        assert_eq!(bank["index"], index);
        assert_eq!(bank["bank_artifact"], bin_name);
        assert_eq!(bank["snapshot_artifact"], snapshot_name);
        let bin = fs::read(fixture.workspace.join(&bin_name)).unwrap();
        let snapshot = fs::read(fixture.workspace.join(&snapshot_name)).unwrap();
        let snapshot_value: ProgramSnapshotV1 = serde_json::from_slice(&snapshot).unwrap();
        aggregate += snapshot.len() as u64;
        assert_eq!(bank["byte_length"], bin.len());
        assert_eq!(bank["bank_sha256"], Sha256Digest::of(&bin).to_hex());
        assert_eq!(
            bank["snapshot_artifact_sha256"],
            Sha256Digest::of(&snapshot).to_hex()
        );
        assert_eq!(
            bank["program_snapshot_sha256"],
            program_snapshot_sha256_v3(&snapshot_value)
                .unwrap()
                .to_hex()
        );
        assert!(bank["bank"]
            .as_str()
            .unwrap()
            .starts_with("recovered_overlay_"));
        let bank_name = bank["bank"].as_str().unwrap();
        if let Some(prior) = prior_bank_name {
            assert!(prior < bank_name, "bank manifest order must be canonical");
        }
        prior_bank_name = Some(bank_name);
        assert!(unique_bank_names.insert(bank_name));
        assert_eq!(bank["backing"]["kind"], "rom_affine");
        assert_eq!(bank["backing"]["rom_space"], "Virtual");
        assert_eq!(
            bank["backing_evidence_fact_indices"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(bank["snapshot_artifact_byte_length"], snapshot.len());
        match bank["ghidra_seeds"]["mode"].as_str().unwrap() {
            "discovery_only" => {
                assert_eq!(bank["ghidra_seeds"]["role"], "candidate_only");
                assert!(bank["ghidra_seeds"].get("base_seed").is_none());
                assert!(bank["ghidra_seeds"].get("snapshot_seed").is_none());
            }
            "base_only" => {
                let base = bank["ghidra_seeds"]["base_seed"].as_u64().unwrap() as u32;
                let va_start = bank["va_start"].as_u64().unwrap() as u32;
                let va_end = bank["va_end"].as_u64().unwrap() as u32;
                assert!(base.is_multiple_of(4) && base >= va_start && base < va_end);
                assert_eq!(bank["ghidra_seeds"]["base_seed_role"], "proven_owner");
            }
            "paired" => {
                let base = bank["ghidra_seeds"]["base_seed"].as_u64().unwrap() as u32;
                let paired = bank["ghidra_seeds"]["snapshot_seed"].as_u64().unwrap() as u32;
                let va_start = bank["va_start"].as_u64().unwrap() as u32;
                let va_end = bank["va_end"].as_u64().unwrap() as u32;
                assert!(base.is_multiple_of(4) && paired.is_multiple_of(4));
                assert!(base >= va_start && base < va_end);
                assert!(paired >= va_start && paired < va_end);
                assert_ne!(base, paired);
                assert_eq!(bank["ghidra_seeds"]["base_seed_role"], "proven_owner");
                assert_eq!(bank["ghidra_seeds"]["snapshot_seed_role"], "assessed_owner");
                assert!(matches!(
                    bank["ghidra_seeds"]["snapshot_seed_assessment"].as_str(),
                    Some("proven" | "candidate" | "ambiguous")
                ));
            }
            mode => panic!("unexpected seed mode {mode}"),
        }
    }
    assert_eq!(manifest["aggregate_snapshot_artifact_bytes"], aggregate);
    assert!(!fixture.workspace.join("input.z64").exists());
    assert_eq!(
        fs::read_dir(&fixture.workspace).unwrap().count(),
        2 * banks.len() + 1
    );

    let second = Fixture::new("recovered-repeat", &recovered_vrom_rom());
    let output = second.run();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut names: Vec<_> = fs::read_dir(&fixture.workspace)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    let mut second_names: Vec<_> = fs::read_dir(&second.workspace)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    second_names.sort();
    assert_eq!(names, second_names);
    for name in names {
        assert_eq!(
            fs::read(fixture.workspace.join(&name)).unwrap(),
            fs::read(second.workspace.join(name)).unwrap()
        );
    }
}

#[test]
fn selected_bank_mode_is_explicit_single_bank_without_cross_bank_authority() {
    let inventory = Fixture::new("selected-inventory", &recovered_vrom_rom());
    let output = inventory.run();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let inventory_manifest: serde_json::Value = serde_json::from_slice(
        &fs::read(inventory.workspace.join("snapshot-workspace.json")).unwrap(),
    )
    .unwrap();
    let selected_name = inventory_manifest["banks"][0]["bank"]
        .as_str()
        .unwrap()
        .to_owned();

    let selected = Fixture::new("selected", &recovered_vrom_rom());
    let output = selected.run_selected(&selected_name);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let manifest: serde_json::Value = serde_json::from_slice(
        &fs::read(selected.workspace.join("snapshot-workspace.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["schema_version"], 4);
    assert_eq!(manifest["state"], "composed");
    assert_eq!(manifest["rom_recompilation_complete"], false);
    assert_eq!(
        manifest["intended_use"],
        "candidate_ghidra_single_bank_only"
    );
    assert_eq!(
        manifest["remaining_recompilation_frontier"],
        "unselected_banks_and_callable_owner_closure"
    );
    assert_eq!(manifest["selection"]["mode"], "single_bank");
    assert_eq!(manifest["selection"]["requested_bank"], selected_name);
    assert_eq!(manifest["selection"]["available_proven_bank_count"], 2);
    assert_eq!(manifest["selection"]["cross_bank_authority"], false);
    assert_eq!(manifest["banks"].as_array().unwrap().len(), 1);
    assert_eq!(manifest["banks"][0]["bank"], selected_name);
    assert_eq!(fs::read_dir(&selected.workspace).unwrap().count(), 3);
}

#[test]
fn selected_bank_mode_rejects_unsafe_or_missing_names_without_publication() {
    for bank in ["..", "missing/bank", "missing"] {
        let fixture = Fixture::new("selected-reject", &recovered_vrom_rom());
        let output = fixture.run_selected(bank);
        assert!(!output.status.success(), "bank {bank} unexpectedly passed");
        assert_eq!(fs::read_dir(&fixture.workspace).unwrap().count(), 0);
        assert!(!fixture.workspace.join("snapshot-workspace.json").exists());
    }
}

#[cfg(unix)]
#[test]
fn rejects_non_private_workspace_and_symlink_rom() {
    let fixture = Fixture::new("boundaries", &open_rom());
    fs::set_permissions(&fixture.workspace, fs::Permissions::from_mode(0o755)).unwrap();
    let output = fixture.run();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("mode 0700"));
    fs::set_permissions(&fixture.workspace, fs::Permissions::from_mode(0o700)).unwrap();

    let alias = fixture.root.join("input-link.z64");
    symlink(&fixture.rom, &alias).unwrap();
    let output = run(&alias, &fixture.workspace);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not a regular file"));
    assert!(!fixture.workspace.join("snapshot-workspace.json").exists());
}

#[test]
fn preexisting_index_artifact_prevents_manifest_publication_and_is_not_replaced() {
    let fixture = Fixture::new("create-new", &recovered_vrom_rom());
    let claimed = fixture.workspace.join("bank-000000.bin");
    fs::write(&claimed, b"winner").unwrap();
    let output = fixture.run();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("reserved snapshot artifact"));
    assert_eq!(fs::read(claimed).unwrap(), b"winner");
    assert!(!fixture.workspace.join("snapshot-workspace.json").exists());
}
