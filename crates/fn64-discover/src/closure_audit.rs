//! Canonical retained execution-closure audit.
//!
//! This module serializes the V3 authority-source closure taxonomy from an already
//! composed snapshot. It does not run discovery, add roots, consult host or
//! runtime catalogs, or mutate proof state. Keeping the serializer here lets a
//! producer emit several diagnostics from one exact in-memory composition
//! without turning the retained audit into a cross-run authority.

use crate::closure::{
    classified_destinations, dynamic_concrete_destination_audit_v1, dynamic_indirect_site_audit_v1,
    scoreboard, unsupported_destination_audit_v1, ClosureScoreboard, DestinationClass,
    DynamicConcreteDestinationAuditV1, DynamicIndirectSiteAuditV1, UnsupportedDestinationAuditV1,
};
use crate::snapshot::{BankInputDigestV1, ProgramSnapshotV1};
use crate::{Fact, NormalizedRom, RomAddressSpace};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::path::Path;

pub const CLOSURE_AUDIT_SCHEMA_V3: &str = "fn64.execution-closure-audit.v3";

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct RetainedClosureAuditV3 {
    schema: &'static str,
    normalized_rom_sha256: String,
    snapshot_schema_versions: Vec<u32>,
    classification_authority: &'static str,
    authorities_not_consulted: [&'static str; 4],
    composed_bank_inputs: Vec<BankInputDigestV1>,
    proven_mapping_geometry: Vec<RetainedRomMappingV1>,
    scoreboard: ClosureScoreboard,
    dynamic_concrete: Vec<DynamicConcreteDestinationAuditV1>,
    dynamic_indirect: Vec<DynamicIndirectSiteAuditV1>,
    unsupported: Vec<UnsupportedDestinationAuditV1>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(deny_unknown_fields)]
struct RetainedRomMappingV1 {
    bank: String,
    rom_space: RomAddressSpace,
    rom_start: u32,
    rom_end: u32,
    va_start: u32,
    va_end: u32,
}

/// Write the V3 audit for one already-composed ROM.
///
/// The returned pair is `(filename, sha256)`. Callers own diagnostics; this
/// function writes no path or ROM-derived content to stdout.
pub fn write_closure_audit_v3(
    label: &str,
    rom: &NormalizedRom,
    snapshots: &[ProgramSnapshotV1],
    audit_dir: &Path,
) -> Result<(String, String), String> {
    let audit = retained_closure_audit_v3(rom, snapshots)?;
    let bytes = serde_json::to_vec_pretty(&audit)
        .map_err(|error| format!("serializing closure audit: {error}"))?;
    std::fs::create_dir_all(audit_dir)
        .map_err(|error| format!("creating closure audit directory: {error}"))?;
    let safe_label = label
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    let filename = format!("{safe_label}.closure-audit-v3.json");
    let path = audit_dir.join(&filename);
    std::fs::write(&path, &bytes)
        .map_err(|error| format!("writing closure audit {}: {error}", path.display()))?;
    let mut sha256 = String::with_capacity(64);
    for byte in Sha256::digest(&bytes) {
        write!(&mut sha256, "{byte:02x}").expect("writing to String cannot fail");
    }
    Ok((filename, sha256))
}

fn retained_closure_audit_v3(
    rom: &NormalizedRom,
    snapshots: &[ProgramSnapshotV1],
) -> Result<RetainedClosureAuditV3, String> {
    let board = scoreboard(snapshots);
    let mut snapshot_schema_versions = snapshots
        .iter()
        .map(|snapshot| snapshot.schema_version)
        .collect::<Vec<_>>();
    snapshot_schema_versions.sort_unstable();
    snapshot_schema_versions.dedup();

    let mut composed_bank_inputs = snapshots
        .iter()
        .flat_map(|snapshot| snapshot.banks.iter().map(|bank| bank.input.clone()))
        .collect::<Vec<_>>();
    composed_bank_inputs.sort_by(|left, right| {
        (
            left.bank.as_str(),
            left.va_start,
            left.va_end,
            &left.backing,
            left.bytes_sha256.as_str(),
        )
            .cmp(&(
                right.bank.as_str(),
                right.va_start,
                right.va_end,
                &right.backing,
                right.bytes_sha256.as_str(),
            ))
    });
    let mut proven_mapping_geometry = snapshots
        .iter()
        .flat_map(|snapshot| snapshot.facts.proven_rom_mappings())
        .filter_map(|fact| match fact {
            Fact::RomMapping {
                bank,
                rom_space,
                rom_start,
                rom_end,
                va_start,
                va_end,
            } => Some(RetainedRomMappingV1 {
                bank: bank.clone(),
                rom_space: *rom_space,
                rom_start: *rom_start,
                rom_end: *rom_end,
                va_start: *va_start,
                va_end: *va_end,
            }),
            _ => None,
        })
        .collect::<Vec<_>>();
    proven_mapping_geometry.sort_unstable();
    proven_mapping_geometry.dedup();

    let dynamic_concrete = dynamic_concrete_destination_audit_v1(snapshots);
    let dynamic_indirect = dynamic_indirect_site_audit_v1(snapshots);
    let unsupported = unsupported_destination_audit_v1(snapshots);
    let expected_dynamic_concrete = classified_destinations(snapshots)
        .into_iter()
        .filter(|destination| destination.class() == DestinationClass::DynamicMips)
        .map(|destination| (destination.va, destination.reason))
        .collect::<Vec<_>>();
    let expected_unsupported = classified_destinations(snapshots)
        .into_iter()
        .filter(|destination| destination.class() == DestinationClass::Unsupported)
        .map(|destination| (destination.va, destination.reason))
        .collect::<Vec<_>>();
    let audit = RetainedClosureAuditV3 {
        schema: CLOSURE_AUDIT_SCHEMA_V3,
        normalized_rom_sha256: rom.sha256.clone(),
        snapshot_schema_versions,
        classification_authority: "union_of_proven_rom_mapping_va_intervals",
        authorities_not_consulted: [
            "abi_issued_host_target_catalog",
            "modeled_exception_vector_image_catalog",
            "canonical_resident_or_overlay_generation_catalog",
            "runtime_tlb_or_kseg_alias_resolution",
        ],
        composed_bank_inputs,
        proven_mapping_geometry,
        scoreboard: board.clone(),
        dynamic_concrete,
        dynamic_indirect,
        unsupported,
    };
    validate_retained_closure_audit_v3(
        &audit,
        &expected_dynamic_concrete,
        &dynamic_indirect_site_audit_v1(snapshots),
        &expected_unsupported,
    )?;
    Ok(audit)
}

fn validate_retained_closure_audit_v3(
    audit: &RetainedClosureAuditV3,
    expected_dynamic_concrete: &[(u32, crate::closure::DestinationReason)],
    expected_dynamic_indirect: &[DynamicIndirectSiteAuditV1],
    expected_unsupported: &[(u32, crate::closure::DestinationReason)],
) -> Result<(), String> {
    let retained_dynamic_concrete = audit
        .dynamic_concrete
        .iter()
        .map(|destination| (destination.destination_va, destination.reason))
        .collect::<Vec<_>>();
    if retained_dynamic_concrete != expected_dynamic_concrete
        || audit
            .dynamic_concrete
            .iter()
            .any(|destination| destination.incoming.is_empty())
    {
        return Err(
            "closure audit did not retain the exact classified dynamic concrete set and incoming edges"
                .to_string(),
        );
    }
    if audit.dynamic_concrete.iter().any(|destination| {
        destination.reason == crate::closure::DestinationReason::ProvenCodeNoOwner
            && destination.block_proof.is_empty()
    }) {
        return Err(
            "closure audit lost block-proof metadata for proven-code dynamic destination"
                .to_string(),
        );
    }
    if audit.dynamic_indirect != expected_dynamic_indirect {
        return Err("closure audit did not retain the exact dynamic indirect set".to_string());
    }
    let retained_dynamic = audit.dynamic_concrete.len() + audit.dynamic_indirect.len();
    if retained_dynamic as u64 != audit.scoreboard.dynamic_mips {
        return Err(format!(
            "closure audit retained {retained_dynamic} dynamic destinations/sites but scoreboard reports {}",
            audit.scoreboard.dynamic_mips
        ));
    }
    let retained_dynamic_bytes = audit.dynamic_concrete.len() as u64 * 4;
    if retained_dynamic_bytes != audit.scoreboard.tally(DestinationClass::DynamicMips).bytes {
        return Err(format!(
            "closure audit retained {retained_dynamic_bytes} dynamic concrete bytes but scoreboard reports {}",
            audit.scoreboard.tally(DestinationClass::DynamicMips).bytes
        ));
    }
    let retained_unsupported = audit
        .unsupported
        .iter()
        .map(|destination| (destination.destination_va, destination.reason))
        .collect::<Vec<_>>();
    if retained_unsupported != expected_unsupported
        || audit
            .unsupported
            .iter()
            .any(|destination| destination.incoming.is_empty())
    {
        return Err(
            "closure audit did not retain the exact classified unsupported set and incoming edges"
                .to_string(),
        );
    }
    if audit.unsupported.len() as u64 != audit.scoreboard.unsupported {
        return Err(format!(
            "closure audit retained {} unsupported destinations but scoreboard reports {}",
            audit.unsupported.len(),
            audit.scoreboard.unsupported
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::closure::{classified_destinations, DestinationClass};
    use crate::facts::{
        executable_range_subject, function_entry_subject, BankAddr, CandidateDetector,
        FunctionEntryEvidence, ProloguePattern, ProofState,
    };
    use crate::snapshot::{compose_materialized_bank_v1, MaterializedBankInput};
    use crate::{normalize, FactDb};
    use std::sync::atomic::{AtomicU64, Ordering};

    const BASE: u32 = 0x8000_0000;
    const ROM_START: u32 = 0x1000;
    const FAR_A: u32 = 0x8080_0000;
    const FAR_B: u32 = 0x8090_0000;
    static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(std::path::PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let ordinal = NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "fn64-closure-audit-test-{}-{ordinal}",
                std::process::id()
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn join(&self, name: &str) -> std::path::PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn asm(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|word| word.to_be_bytes()).collect()
    }

    fn synthetic_rom(bank: &[u8]) -> NormalizedRom {
        let mut bytes = vec![0u8; ROM_START as usize + bank.len()];
        bytes[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        bytes[8..12].copy_from_slice(&BASE.to_be_bytes());
        bytes[ROM_START as usize..].copy_from_slice(bank);
        normalize(&bytes).unwrap()
    }

    fn synthetic_facts(byte_len: u32, entries: &[u32]) -> FactDb {
        let mut facts = FactDb::new();
        let mapping = facts.insert(Fact::RomMapping {
            bank: "bank".into(),
            rom_space: RomAddressSpace::Physical,
            rom_start: ROM_START,
            rom_end: ROM_START + byte_len,
            va_start: BASE,
            va_end: BASE + byte_len,
        });
        facts
            .conclude("bank:bank", ProofState::Proven, vec![mapping], "test")
            .unwrap();
        let executable = facts.insert(Fact::ExecutableRange {
            bank: "bank".into(),
            va_start: BASE,
            va_end: BASE + byte_len,
        });
        facts
            .conclude(
                executable_range_subject("bank", BASE, BASE + byte_len),
                ProofState::Proven,
                vec![executable],
                "test",
            )
            .unwrap();
        for &entry in entries {
            let target = BankAddr::new("bank", entry);
            let claim = facts.insert(Fact::FunctionEntryClaim {
                target: target.clone(),
                detector: CandidateDetector::ProloguePattern,
                evidence: FunctionEntryEvidence::Prologue {
                    stack_adjust: target.clone(),
                    frame_size: 16,
                    pattern: ProloguePattern::LeafWithMatchedRestore,
                    corroborating_site: BankAddr::new("bank", entry + 4),
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
        facts
    }

    fn synthetic_snapshot() -> (NormalizedRom, ProgramSnapshotV1) {
        let jump = |target: u32| 0x0800_0000 | (target >> 2) & 0x03ff_ffff;
        let bytes = asm(&[jump(FAR_B), 0, jump(FAR_A), 0, jump(FAR_B), 0]);
        let rom = synthetic_rom(&bytes);
        let entries = [BASE, BASE + 8, BASE + 16];
        let facts = synthetic_facts(bytes.len() as u32, &entries);
        let snapshot = compose_materialized_bank_v1(
            &rom,
            &facts,
            MaterializedBankInput {
                bank: "bank",
                va_start: BASE,
                bytes: &bytes,
                seed_roots: &entries,
            },
        )
        .unwrap();
        (rom, snapshot)
    }

    fn synthetic_dynamic_snapshot() -> (NormalizedRom, ProgramSnapshotV1) {
        // BASE branches to a far-jump block; BASE+8 is an open `jr $t0`.
        let far = 0x8090_0000u32;
        let branch_to_jump = 0x1000_0003;
        let jr_t0 = 0x0100_0008;
        let jump_far = 0x0800_0000 | (far >> 2) & 0x03ff_ffff;
        let bytes = asm(&[branch_to_jump, 0, jr_t0, 0, jump_far, 0]);
        let rom = synthetic_rom(&bytes);
        let facts = synthetic_facts(bytes.len() as u32, &[BASE]);
        let mut snapshot = compose_materialized_bank_v1(
            &rom,
            &facts,
            MaterializedBankInput {
                bank: "bank",
                va_start: BASE,
                bytes: &bytes,
                seed_roots: &[BASE],
            },
        )
        .unwrap();
        let assessment = snapshot.banks[0]
            .block_proof
            .assessments
            .iter_mut()
            .find(|assessment| {
                matches!(
                    assessment,
                    crate::block_proof::BlockAssessment::Proven { block }
                        if block.start_va == BASE + 0x10
                )
            })
            .expect("branch target begins a proven block");
        *assessment = crate::block_proof::BlockAssessment::Candidate {
            start_va: BASE + 0x10,
            end_va: BASE + 0x18,
            blockers: vec![crate::block_proof::BlockProofBlocker::Unowned],
        };
        snapshot.banks[0].block_proof.proven_blocks -= 1;
        snapshot.banks[0].block_proof.proven_bytes -= 8;
        (rom, snapshot)
    }

    fn expected_unsupported(
        snapshot: &ProgramSnapshotV1,
    ) -> Vec<(u32, crate::closure::DestinationReason)> {
        classified_destinations(std::slice::from_ref(snapshot))
            .into_iter()
            .filter(|destination| destination.class() == DestinationClass::Unsupported)
            .map(|destination| (destination.va, destination.reason))
            .collect()
    }

    #[test]
    fn writes_canonical_filename_schema_field_order_and_digest() {
        let (rom, snapshot) = synthetic_snapshot();
        let directory = TestDirectory::new();
        let output = directory.join("audit");
        let (filename, digest) =
            write_closure_audit_v3("MiXeD Label!", &rom, &[snapshot], &output).unwrap();
        assert_eq!(filename, "mixed-label-.closure-audit-v3.json");
        let bytes = std::fs::read(output.join(&filename)).unwrap();
        assert_eq!(digest, format!("{:x}", Sha256::digest(&bytes)));
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["schema"], CLOSURE_AUDIT_SCHEMA_V3);
        assert_eq!(value["normalized_rom_sha256"], rom.sha256);
        assert_eq!(
            value["composed_bank_inputs"][0]["backing"]["kind"],
            "rom_affine"
        );

        let text = std::str::from_utf8(&bytes).unwrap();
        let fields = [
            "\n  \"schema\"",
            "\n  \"normalized_rom_sha256\"",
            "\n  \"snapshot_schema_versions\"",
            "\n  \"classification_authority\"",
            "\n  \"authorities_not_consulted\"",
            "\n  \"composed_bank_inputs\"",
            "\n  \"proven_mapping_geometry\"",
            "\n  \"scoreboard\"",
            "\n  \"dynamic_concrete\"",
            "\n  \"dynamic_indirect\"",
            "\n  \"unsupported\"",
        ];
        let positions = fields.map(|field| text.find(field).expect("canonical field missing"));
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn canonical_output_ignores_cfg_unsupported_and_incoming_iteration_order() {
        let (rom, snapshot) = synthetic_snapshot();
        let mut reordered = snapshot.clone();
        reordered.banks[0].closure.cfg.blocks.reverse();
        let directory = TestDirectory::new();
        let first = directory.join("first");
        let second = directory.join("second");
        let (_, first_digest) = write_closure_audit_v3("same", &rom, &[snapshot], &first).unwrap();
        let (_, second_digest) =
            write_closure_audit_v3("same", &rom, &[reordered], &second).unwrap();
        assert_eq!(first_digest, second_digest);
        assert_eq!(
            std::fs::read(first.join("same.closure-audit-v3.json")).unwrap(),
            std::fs::read(second.join("same.closure-audit-v3.json")).unwrap()
        );
    }

    #[test]
    fn retains_exact_dynamic_details_without_instruction_words() {
        let (rom, snapshot) = synthetic_dynamic_snapshot();
        let audit = retained_closure_audit_v3(&rom, std::slice::from_ref(&snapshot)).unwrap();
        assert_eq!(audit.scoreboard.dynamic_mips, 2);
        assert_eq!(audit.dynamic_concrete.len(), 1);
        let concrete = &audit.dynamic_concrete[0];
        assert_eq!(concrete.destination_va, BASE + 0x10);
        assert_eq!(
            concrete.reason,
            crate::closure::DestinationReason::ProvenCodeNoOwner
        );
        assert_eq!(concrete.incoming.len(), 1);
        assert_eq!(concrete.block_proof.len(), 1);
        assert_eq!(concrete.block_proof[0].blocker_kinds, ["unowned"]);
        assert!(!concrete.owner_proof.is_empty());

        assert_eq!(audit.dynamic_indirect.len(), 1);
        let indirect = &audit.dynamic_indirect[0];
        assert_eq!(indirect.bank, "bank");
        assert_eq!(indirect.site_pc, BASE + 8);
        assert!(!indirect.via_call);
        assert_eq!(indirect.state, crate::resolve::IndirectProofState::Open);
        assert_eq!(indirect.kind, None);
        assert!(indirect.targets.is_empty());
        assert!(indirect.memory_sources.is_empty());

        let dynamic_json =
            serde_json::to_value((&audit.dynamic_concrete, &audit.dynamic_indirect)).unwrap();
        assert!(dynamic_json.to_string().find("\"word\"").is_none());
    }

    #[test]
    fn dynamic_projection_is_canonical_and_must_match_the_scoreboard() {
        let (rom, snapshot) = synthetic_dynamic_snapshot();
        let expected_concrete = classified_destinations(std::slice::from_ref(&snapshot))
            .into_iter()
            .filter(|destination| destination.class() == DestinationClass::DynamicMips)
            .map(|destination| (destination.va, destination.reason))
            .collect::<Vec<_>>();
        let expected_indirect = dynamic_indirect_site_audit_v1(std::slice::from_ref(&snapshot));

        let mut reordered = snapshot.clone();
        reordered.banks[0].authority_closure.indirect.reverse();
        reordered.banks[0].block_proof.assessments.reverse();
        reordered.banks[0].owner_proof.assessments.reverse();
        assert_eq!(
            dynamic_concrete_destination_audit_v1(std::slice::from_ref(&snapshot)),
            dynamic_concrete_destination_audit_v1(std::slice::from_ref(&reordered))
        );
        assert_eq!(
            expected_indirect,
            dynamic_indirect_site_audit_v1(std::slice::from_ref(&reordered))
        );

        let mut missing_concrete =
            retained_closure_audit_v3(&rom, std::slice::from_ref(&snapshot)).unwrap();
        missing_concrete.dynamic_concrete.clear();
        let error = validate_retained_closure_audit_v3(
            &missing_concrete,
            &expected_concrete,
            &expected_indirect,
            &expected_unsupported(&snapshot),
        )
        .unwrap_err();
        assert!(error.contains("exact classified dynamic concrete set"));

        let mut missing_indirect =
            retained_closure_audit_v3(&rom, std::slice::from_ref(&snapshot)).unwrap();
        missing_indirect.dynamic_indirect.clear();
        let error = validate_retained_closure_audit_v3(
            &missing_indirect,
            &expected_concrete,
            &expected_indirect,
            &expected_unsupported(&snapshot),
        )
        .unwrap_err();
        assert!(error.contains("exact dynamic indirect set"));
    }

    #[test]
    fn rejects_scoreboard_and_unsupported_projection_mismatches() {
        let (rom, snapshot) = synthetic_snapshot();
        let expected = expected_unsupported(&snapshot);

        let mut scoreboard_mismatch =
            retained_closure_audit_v3(&rom, std::slice::from_ref(&snapshot)).unwrap();
        scoreboard_mismatch.scoreboard.unsupported += 1;
        let error = validate_retained_closure_audit_v3(&scoreboard_mismatch, &[], &[], &expected)
            .unwrap_err();
        assert!(error.contains("scoreboard reports"));

        let mut unsupported_mismatch =
            retained_closure_audit_v3(&rom, std::slice::from_ref(&snapshot)).unwrap();
        unsupported_mismatch.unsupported.pop();
        let error = validate_retained_closure_audit_v3(&unsupported_mismatch, &[], &[], &expected)
            .unwrap_err();
        assert!(error.contains("exact classified unsupported set"));
    }

    #[test]
    fn fails_when_output_directory_path_is_an_existing_file() {
        let (rom, snapshot) = synthetic_snapshot();
        let directory = TestDirectory::new();
        let blocked = directory.join("not-a-directory");
        std::fs::write(&blocked, b"sentinel").unwrap();
        let error = write_closure_audit_v3("blocked", &rom, &[snapshot], &blocked).unwrap_err();
        assert!(error.contains("creating closure audit directory"));
        assert_eq!(std::fs::read(blocked).unwrap(), b"sentinel");
    }
}
