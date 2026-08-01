//! End-to-end wire fixtures for receipt-bound discovery-only tool claims.
//!
//! Everything here is synthetic: no game image or extracted game output is
//! read or retained by the test.

use fn64_discover::banks::BOOT_BANK;
use fn64_discover::candidate_corroboration::{
    validate_discovery_only_tool_claims_v1, CandidateCorroborationError, DiscoveryOnlyReceiptBundle,
};
use fn64_discover::candidate_relation_report::{
    probe_baseline_unreached_candidates_v1, report_candidate_native_relations_v1,
};
use fn64_discover::facts::{
    executable_range_subject, BankAddr, CandidateDetector, Fact, FactDb, FunctionEntryEvidence,
    ProloguePattern, ProofState, RomAddressSpace,
};
use fn64_discover::snapshot::{compose_materialized_bank_v1, MaterializedBankInput};
use fn64_discover::tool_adapter::{
    canonical_claim_records_sha256, export_complete_tool_jsonl_v2, ingest_tool_jsonl,
    AdapterLimits, CompleteToolRun, Sha256Digest, ToolAdapterExpectation, ToolCandidateKind,
    ToolClaimRecord, ToolIdentity, ToolLineageRef, ToolLineageRole, ToolResourceDiagnostics,
    ToolRunRole,
};
use fn64_discover::tool_claims::{
    bank_input_identity_v1, discovery_snapshot_lineage_v3, freeze_tool_claims_v1,
    program_snapshot_sha256_v3,
};
use serde_json::{json, Value};

const BASE: u32 = 0x8000_0400;

struct Fixture {
    queue_request: Vec<u8>,
    terminal_queue_receipt: Vec<u8>,
    bank_attempt_result: Vec<u8>,
    runner_receipt: Vec<u8>,
    runner_request: Vec<u8>,
    evidence: Vec<u8>,
    unseeded_config: Vec<u8>,
    unseeded_tool_manifest: Vec<u8>,
    provider_jsonl: Vec<u8>,
    tool_claims: Vec<u8>,
    snapshot: Vec<u8>,
    bank_bytes: Vec<u8>,
}

impl Fixture {
    fn bundle(&self) -> DiscoveryOnlyReceiptBundle<'_> {
        DiscoveryOnlyReceiptBundle {
            queue_request: &self.queue_request,
            terminal_queue_receipt: &self.terminal_queue_receipt,
            bank_attempt_result: &self.bank_attempt_result,
            runner_receipt: &self.runner_receipt,
            runner_request: &self.runner_request,
            evidence: &self.evidence,
            unseeded_config: &self.unseeded_config,
            unseeded_tool_manifest: &self.unseeded_tool_manifest,
            provider_jsonl: &self.provider_jsonl,
            tool_claims: &self.tool_claims,
            snapshot: &self.snapshot,
            bank_index: 0,
        }
    }
}

fn digest(bytes: &[u8]) -> String {
    Sha256Digest::of(bytes).to_hex()
}

fn bytes(value: Value) -> Vec<u8> {
    serde_json::to_vec(&value).unwrap()
}

fn file_identity(bytes: &[u8]) -> Value {
    json!({"byte_length": bytes.len(), "sha256": digest(bytes)})
}

fn fixture() -> Fixture {
    let mut image = vec![0_u8; 0x1040];
    image[..4].copy_from_slice(&0x8037_1240_u32.to_be_bytes());
    image[8..12].copy_from_slice(&BASE.to_be_bytes());
    for (index, word) in [
        0x0c00_0104_u32,
        0x0000_0000,
        0x03e0_0008,
        0x0000_0000,
        0x03e0_0008,
        0x0000_0000,
    ]
    .into_iter()
    .enumerate()
    {
        image[0x1000 + index * 4..0x1004 + index * 4].copy_from_slice(&word.to_be_bytes());
    }
    let rom = fn64_discover::normalize(&image).unwrap();
    let bank_bytes = rom.bytes[0x1000..].to_vec();
    let mut facts = FactDb::new();
    let mapping = facts.insert(Fact::RomMapping {
        bank: BOOT_BANK.into(),
        rom_space: RomAddressSpace::Physical,
        rom_start: 0x1000,
        rom_end: 0x1040,
        va_start: BASE,
        va_end: BASE + 0x40,
    });
    facts
        .conclude(
            "bank:boot",
            ProofState::Proven,
            vec![mapping],
            "candidate_corroboration_fixture",
        )
        .unwrap();
    let executable = facts.insert(Fact::ExecutableRange {
        bank: BOOT_BANK.into(),
        va_start: BASE,
        va_end: BASE + 0x40,
    });
    facts
        .conclude(
            executable_range_subject(BOOT_BANK, BASE, BASE + 0x40),
            ProofState::Proven,
            vec![executable],
            "candidate_corroboration_fixture",
        )
        .unwrap();
    let target = BankAddr::new(BOOT_BANK, BASE);
    let entry = facts.insert(Fact::FunctionEntryClaim {
        target: target.clone(),
        detector: CandidateDetector::ProloguePattern,
        evidence: FunctionEntryEvidence::Prologue {
            stack_adjust: target.clone(),
            frame_size: 16,
            pattern: ProloguePattern::LeafWithMatchedRestore,
            corroborating_site: BankAddr::new(BOOT_BANK, BASE + 4),
        },
        proposed_state: ProofState::Proven,
    });
    facts
        .conclude(
            fn64_discover::facts::function_entry_subject(&target),
            ProofState::Proven,
            vec![entry],
            "candidate_corroboration_fixture",
        )
        .unwrap();
    let snapshot = compose_materialized_bank_v1(
        &rom,
        &facts,
        MaterializedBankInput {
            bank: BOOT_BANK,
            va_start: BASE,
            bytes: &bank_bytes,
            seed_roots: &[BASE],
        },
    )
    .unwrap();
    let snapshot_bytes = serde_json::to_vec(&snapshot).unwrap();
    let snapshot_sha = program_snapshot_sha256_v3(&snapshot).unwrap().to_hex();
    let input = bank_input_identity_v1(&snapshot, BOOT_BANK).unwrap();

    let source_manifest = b"synthetic-source-manifest";
    let distribution = b"synthetic-ghidra-distribution";
    let distribution_sha = digest(distribution);
    let tool_artifacts = [
        "tool-artifacts/Fn64ExportCandidates.java",
        "tool-artifacts/analyzeHeadless",
        "tool-artifacts/application.properties",
        "tool-artifacts/ghidra-distribution.json",
        "tool-artifacts/java",
        "tool-artifacts/orchestration.json",
    ]
    .into_iter()
    .enumerate()
    .map(|(index, path)| {
        json!({
            "path": path,
            "byte_length": index + 1,
            "sha256": if path == "tool-artifacts/ghidra-distribution.json" {
                distribution_sha.clone()
            } else {
                digest(format!("synthetic-tool-{index}").as_bytes())
            },
        })
    })
    .collect::<Vec<_>>();
    let tool_manifest = bytes(json!({
        "schema": "fn64.tool-artifact-manifest",
        "schema_version": 1,
        "tool_name": "ghidra-headless-unseeded",
        "tool_version": "11.synthetic",
        "artifacts": tool_artifacts,
    }));
    let tool_sha = digest(&tool_manifest);
    let config = bytes(json!({
        "schema": "fn64.ghidra-bank-config",
        "schema_version": 1,
        "mode": "unseeded",
        "bank": BOOT_BANK,
        "va_start": input.va_start,
        "va_end": input.va_end,
        "base_seed": Value::Null,
        "snapshot_seed": Value::Null,
        "loader": "BinaryLoader",
        "processor": "MIPS:BE:64:64-32addr",
        "cspec": "o32",
        "ghidra_version": "11.synthetic",
        "analysis_timeout_seconds": 1,
        "max_cpu": 1,
        "heap_mib": 1,
        "rss_mib": 1,
        "min_free_percent": 1,
        "wall_seconds": 1,
        "tool_manifest_sha256": tool_sha,
        "role": "candidate_only",
    }));
    let evidence = bytes(json!({
        "schema": "fn64.snapshot-bank-evidence",
        "schema_version": 3,
        "program_snapshot_sha256": snapshot_sha,
        "input": input,
        "backing": {
            "rom_space": "Physical",
            "rom_start": 0x1000,
            "rom_end": 0x1040,
        },
        "artifact": file_identity(&bank_bytes),
        "seeds": {"mode": "discovery_only", "role": "candidate_only"},
    }));
    let lineage = vec![
        discovery_snapshot_lineage_v3(&snapshot).unwrap(),
        ToolLineageRef {
            role: ToolLineageRole::EvidenceManifest,
            source_sha256: Sha256Digest::of(&evidence),
        },
        ToolLineageRef {
            role: ToolLineageRole::ToolConfiguration,
            source_sha256: Sha256Digest::of(&config),
        },
    ];
    let role = ToolRunRole::FunctionBoundaryCandidates;
    let provider_jsonl = export_complete_tool_jsonl_v2(CompleteToolRun {
        tool: ToolIdentity {
            name: "ghidra-headless-unseeded".into(),
            version: "11.synthetic".into(),
            build_sha256: Sha256Digest::of(&tool_manifest),
        },
        role: role.clone(),
        input: input.clone(),
        lineage: lineage.clone(),
        claims: vec![ToolClaimRecord {
            sequence: 0,
            provider_claim_id: "synthetic-entry".into(),
            claim: ToolCandidateKind::FunctionEntry {
                address: BankAddr::new(BOOT_BANK, BASE),
            },
        }],
        resources: ToolResourceDiagnostics {
            input_bytes: bank_bytes.len() as u64,
            elapsed_millis: 1,
            peak_memory_bytes: None,
            limit_hit: false,
            warnings: Vec::new(),
        },
    })
    .unwrap()
    .into_bytes();
    let output = ingest_tool_jsonl(
        &String::from_utf8(provider_jsonl.clone()).unwrap(),
        &ToolAdapterExpectation {
            input: input.clone(),
            role: role.clone(),
            lineage: lineage.clone(),
            limits: AdapterLimits::default(),
        },
    )
    .unwrap();
    let tool_claims =
        serde_json::to_vec(&freeze_tool_claims_v1(&snapshot, [&output]).unwrap()).unwrap();
    let runner_request = bytes(json!({
        "schema": "fn64.tool-ingest-request",
        "schema_version": 1,
        "runs": [{
            "bank": BOOT_BANK,
            "jsonl": "modes/unseeded/out/provider.jsonl",
            "lineage_artifacts": [
                {"path": "config/unseeded.json", "role": "tool_configuration"},
                {"path": "raw/evidence.json", "role": "evidence_manifest"},
            ],
            "role": "function_boundary_candidates",
            "tool": {
                "name": "ghidra-headless-unseeded",
                "version": "11.synthetic",
                "build_sha256": tool_sha,
            },
            "tool_artifact_manifest": "tool-unseeded.json",
        }],
    }));
    let nonzero = digest(b"synthetic-resource-evidence");
    let runner_receipt = bytes(json!({
        "schema": "fn64.ghidra-snapshot-bank-receipt",
        "schema_version": 1,
        "execution_mode": "discovery-only",
        "paired_comparison_complete": false,
        "completed_modes": ["unseeded"],
        "program_snapshot_sha256": snapshot_sha,
        "bank": BOOT_BANK,
        "seeds": {"mode": "discovery_only", "role": "candidate_only"},
        "evidence_sha256": digest(&evidence),
        "request_sha256": digest(&runner_request),
        "unseeded_tool_manifest_sha256": digest(&tool_manifest),
        "tool_claims_sha256": digest(&tool_claims),
        "ghidra_distribution_manifest_complete": true,
        "ghidra_distribution_manifest_sha256": distribution_sha,
        "ghidra_distribution_file_count": 1,
        "tool_artifact_scope": "all-ghidra-install-regular-files,jdk-java,fn64-analysis-scripts,and-bound-orchestration-helpers",
        "configuration_sha256": {"unseeded": digest(&config)},
        "provider_jsonl_sha256": {"unseeded": digest(&provider_jsonl)},
        "resource_evidence_sha256": {
            "ghidra_distribution_scan_log": nonzero,
            "ghidra_distribution_scan": nonzero,
            "ghidra_distribution_verify_log": nonzero,
            "ghidra_distribution_verify": nonzero,
            "stage": nonzero,
            "unseeded": nonzero,
            "ingest": nonzero,
        },
    }));
    let queue_request = bytes(json!({
        "schema": "fn64.ghidra-snapshot-workspace-request",
        "schema_version": 1,
        "source_manifest_sha256": digest(source_manifest),
        "normalized_rom_sha256": input.normalized_rom_sha256,
        "execution_mode": "candidate-only-sequential",
        "tools": {
            "queue": file_identity(b"queue"),
            "runner": file_identity(b"runner"),
            "stage": file_identity(b"stage"),
            "ingest": file_identity(b"ingest"),
        },
        "caps": {
            "max_launches": 1,
            "max_wall_seconds": 1,
            "max_attempts_per_bank": 1,
            "max_ordinary_failures": 1,
            "max_log_bytes": 1,
            "max_attempt_bytes": 1,
            "min_free_disk_bytes": 1,
            "termination_grace_seconds": 1,
        },
    }));
    let attempt = bytes(json!({
        "schema": "fn64.ghidra-snapshot-workspace-attempt",
        "schema_version": 1,
        "state": "success",
        "failure_class": Value::Null,
        "runner_exit_status": 0,
        "runner_attempt": "synthetic-attempt",
        "runner_receipt_sha256": digest(&runner_receipt),
        "tool_claims_sha256": digest(&tool_claims),
        "ghidra_distribution_manifest_sha256": digest(distribution),
        "unseeded_tool_manifest_sha256": digest(&tool_manifest),
        "common_cohort_sha256": digest(b"synthetic-cohort"),
        "stop_scheduling": false,
        "stdout": file_identity(b"stdout"),
        "stderr": file_identity(b"stderr"),
        "queue_request_sha256": digest(&queue_request),
        "source_manifest_sha256": digest(source_manifest),
        "attempt": 1,
        "bank": {
            "index": 0,
            "name": BOOT_BANK,
            "bank_sha256": input.bank_bytes_sha256,
            "snapshot_artifact_sha256": digest(&snapshot_bytes),
            "program_snapshot_sha256": snapshot_sha,
            "base_seed": Value::Null,
        },
    }));
    let terminal_queue_receipt = bytes(json!({
        "schema": "fn64.ghidra-snapshot-workspace-receipt",
        "schema_version": 1,
        "state": "candidate_queue_complete",
        "execution_mode": "candidate-only-sequential",
        "queue_request_sha256": digest(&queue_request),
        "source_manifest_sha256": digest(source_manifest),
        "normalized_rom_sha256": input.normalized_rom_sha256,
        "cohort": {
            "common_sha256": digest(b"synthetic-cohort"),
            "ghidra_distribution_manifest_sha256": digest(distribution),
            "ghidra_distribution_file_count": 1,
            "tool_artifact_scope": "all-ghidra-install-regular-files,jdk-java,fn64-analysis-scripts,and-bound-orchestration-helpers",
            "mode_tool_manifest_sha256": {"discovery_only": digest(&tool_manifest), "base_only": Value::Null},
        },
        "banks": [{"index": 0, "state": "success", "receipt_sha256": digest(&attempt)}],
    }));
    Fixture {
        queue_request,
        terminal_queue_receipt,
        bank_attempt_result: attempt,
        runner_receipt,
        runner_request,
        evidence,
        unseeded_config: config,
        unseeded_tool_manifest: tool_manifest,
        provider_jsonl,
        tool_claims,
        snapshot: snapshot_bytes,
        bank_bytes,
    }
}

#[test]
fn accepts_only_an_exact_synthetic_discovery_only_receipt_chain() {
    let fixture = fixture();
    let admitted = validate_discovery_only_tool_claims_v1(fixture.bundle()).unwrap();
    assert_eq!(admitted.bank_index(), 0);
    assert_eq!(admitted.claims().claims.len(), 1);
    assert_eq!(
        admitted.analyzer_completeness(),
        fn64_discover::candidate_corroboration::AnalyzerCompleteness::Unknown
    );
    let relations = report_candidate_native_relations_v1(&admitted).unwrap();
    assert_eq!(relations.candidate_entries, 1);
    assert_eq!(relations.snapshot_entry_states.proven, 1);
    assert_eq!(relations.baseline_reached, 1);
    assert_eq!(relations.baseline_unreached, 0);
    assert_eq!(
        relations.baseline_unreached_snapshot_entry_states,
        Default::default()
    );
    assert_eq!(relations.baseline_proven_code_direct_call_targets, 0);
    assert_eq!(relations.baseline_exhaustive_resolved_call_targets, 0);
    assert_eq!(relations.baseline_reached_without_call_relation, 1);
    let probe = probe_baseline_unreached_candidates_v1(&admitted, &fixture.bank_bytes).unwrap();
    assert_eq!(probe.selected_unreached_roots, 0);
    assert_eq!(probe.visited_words, 0);
    assert_eq!(probe.new_words, 0);
}

#[test]
fn rejects_tampered_receipt_provider_snapshot_and_empty_claims() {
    let fixture = fixture();
    let mut runner_receipt = fixture.runner_receipt.clone();
    runner_receipt[0] ^= 1;
    assert!(
        validate_discovery_only_tool_claims_v1(DiscoveryOnlyReceiptBundle {
            runner_receipt: &runner_receipt,
            ..fixture.bundle()
        })
        .is_err()
    );

    let mut provider_jsonl = fixture.provider_jsonl.clone();
    provider_jsonl[0] ^= 1;
    assert!(
        validate_discovery_only_tool_claims_v1(DiscoveryOnlyReceiptBundle {
            provider_jsonl: &provider_jsonl,
            ..fixture.bundle()
        })
        .is_err()
    );

    let mut snapshot = fixture.snapshot.clone();
    snapshot[0] ^= 1;
    assert!(
        validate_discovery_only_tool_claims_v1(DiscoveryOnlyReceiptBundle {
            snapshot: &snapshot,
            ..fixture.bundle()
        })
        .is_err()
    );

    let mut empty_claims: Value = serde_json::from_slice(&fixture.tool_claims).unwrap();
    empty_claims["claims"] = json!([]);
    let empty_claims = bytes(empty_claims);
    assert!(
        validate_discovery_only_tool_claims_v1(DiscoveryOnlyReceiptBundle {
            tool_claims: &empty_claims,
            ..fixture.bundle()
        })
        .is_err()
    );
}

#[test]
fn rejects_extra_lineage_and_distribution_mismatch() {
    let fixture = fixture();
    let mut runner_request: Value = serde_json::from_slice(&fixture.runner_request).unwrap();
    runner_request["runs"][0]["lineage_artifacts"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "path": "unexpected.json",
            "role": "provider_output",
        }));
    let runner_request = bytes(runner_request);
    assert!(
        validate_discovery_only_tool_claims_v1(DiscoveryOnlyReceiptBundle {
            runner_request: &runner_request,
            ..fixture.bundle()
        })
        .is_err()
    );

    let mut tool_manifest: Value = serde_json::from_slice(&fixture.unseeded_tool_manifest).unwrap();
    tool_manifest["artifacts"][3]["sha256"] = json!(digest(b"different-distribution"));
    let tool_manifest = bytes(tool_manifest);
    assert!(
        validate_discovery_only_tool_claims_v1(DiscoveryOnlyReceiptBundle {
            unseeded_tool_manifest: &tool_manifest,
            ..fixture.bundle()
        })
        .is_err()
    );
}

#[test]
fn rejects_semantically_divergent_provider_after_all_receipt_hashes_are_rebound() {
    let fixture = fixture();
    let mut records: Vec<Value> = String::from_utf8(fixture.provider_jsonl.clone())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    records[1]["claim"]["address"]["pc"] = json!(BASE + 0x10);
    let changed_claim = ToolClaimRecord {
        sequence: records[1]["sequence"].as_u64().unwrap(),
        provider_claim_id: records[1]["provider_claim_id"].as_str().unwrap().into(),
        claim: serde_json::from_value(records[1]["claim"].clone()).unwrap(),
    };
    records[2]["claims_sha256"] = json!(canonical_claim_records_sha256(&[changed_claim]).to_hex());
    let provider_jsonl = records
        .iter()
        .map(|record| serde_json::to_string(record).unwrap())
        .collect::<Vec<_>>()
        .join("\n")
        .into_bytes();

    let mut runner_receipt: Value = serde_json::from_slice(&fixture.runner_receipt).unwrap();
    runner_receipt["provider_jsonl_sha256"]["unseeded"] = json!(digest(&provider_jsonl));
    let runner_receipt = bytes(runner_receipt);

    let mut attempt: Value = serde_json::from_slice(&fixture.bank_attempt_result).unwrap();
    attempt["runner_receipt_sha256"] = json!(digest(&runner_receipt));
    let attempt = bytes(attempt);

    let mut terminal: Value = serde_json::from_slice(&fixture.terminal_queue_receipt).unwrap();
    terminal["banks"][0]["receipt_sha256"] = json!(digest(&attempt));
    let terminal = bytes(terminal);

    let error = validate_discovery_only_tool_claims_v1(DiscoveryOnlyReceiptBundle {
        terminal_queue_receipt: &terminal,
        bank_attempt_result: &attempt,
        runner_receipt: &runner_receipt,
        provider_jsonl: &provider_jsonl,
        ..fixture.bundle()
    })
    .unwrap_err();
    assert_eq!(
        error,
        CandidateCorroborationError::ToolClaims(
            "claim sidecar does not equal the retained provider JSONL replay".into()
        )
    );
}
