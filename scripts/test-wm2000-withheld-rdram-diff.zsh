#!/bin/zsh

set -eu

typeset -r test_root=${0:A:h:h}
typeset -r test_dir=$(mktemp -d /private/tmp/fn64-wm-withheld-diff-test.XXXXXX)
trap 'rm -rf -- "$test_dir"' EXIT

typeset -r test_rom=$test_dir/rom.z64
typeset -r test_boot=$test_dir/boot.json
typeset -r test_schedule=$test_dir/schedule.json
typeset -r test_aot=$test_dir/aot
typeset -r test_dynamic=$test_dir/dynamic
typeset -r test_receipt=$test_dir/receipt.json
typeset -r test_v3_receipt=$test_dir/receipt-v3.json
typeset -r test_mixed_v4_receipt=$test_dir/receipt-mixed-v4.json
typeset -r test_stale_receipt=$test_dir/stale-receipt.json
typeset -r test_unpaired_receipt=$test_dir/unpaired-receipt.json
typeset -r test_output=$test_dir/output
typeset -r test_v3_output=$test_dir/v3-output
typeset -r test_mixed_v4_output=$test_dir/mixed-v4-output
typeset -r test_stale_output=$test_dir/stale-output
typeset -r test_component_output=$test_dir/component-output
typeset -r test_noncomparable_output=$test_dir/noncomparable-output
typeset -r test_parked_fault_output=$test_dir/parked-fault-output
typeset -r test_cpu_mismatch_output=$test_dir/cpu-mismatch-output
typeset -r test_continuation_mismatch_output=$test_dir/continuation-mismatch-output
typeset -r test_stale_receipt_output=$test_dir/stale-receipt-output
typeset -r test_unpaired_receipt_output=$test_dir/unpaired-receipt-output
typeset -r test_mutation_output=$test_dir/mutation-output
typeset -r test_wrong_pc_output=$test_dir/wrong-pc-output
typeset -r test_identity_drift_output=$test_dir/identity-drift-output
typeset -r test_zero_dynamic_work_output=$test_dir/zero-dynamic-work-output
typeset -r test_v1_telemetry_output=$test_dir/v1-telemetry-output
typeset -r test_resolver_drift_output=$test_dir/resolver-drift-output
mkdir "$test_output"
mkdir "$test_v3_output"
mkdir "$test_mixed_v4_output"
mkdir "$test_stale_output"
mkdir "$test_component_output"
mkdir "$test_noncomparable_output"
mkdir "$test_parked_fault_output"
mkdir "$test_cpu_mismatch_output"
mkdir "$test_continuation_mismatch_output"
mkdir "$test_stale_receipt_output"
mkdir "$test_unpaired_receipt_output"
mkdir "$test_mutation_output"
mkdir "$test_wrong_pc_output"
mkdir "$test_identity_drift_output"
mkdir "$test_zero_dynamic_work_output"
mkdir "$test_v1_telemetry_output"
mkdir "$test_resolver_drift_output"
printf '\x80\x37\x12\x40rom' >"$test_rom"
print -n -- boot >"$test_boot"
print -n -- schedule >"$test_schedule"

cat >"$test_aot" <<'EOF'
#!/bin/zsh
print -- '[wm2000-program-identity] schema=fn64.wm2000.program-identity.v1 sha256=5555555555555555555555555555555555555555555555555555555555555555 source=canonical_block_program_sha256 resolver_sha256=1111111111111111111111111111111111111111111111111111111111111111 entry_bank=0000000000000001 entry_pc=80001000'
print -- '[wm2000-block-checkpoint] minimum_guest_instructions=1000000 expected_guest_instructions=None achieved_guest_instructions=1000004 scheduler_steps=42 sim_time=9001 logical_rdram_bytes=8388608 logical_rdram_sha256=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'
if [[ "${FN64_TEST_PARKED_FAULT:-}" == 1 ]]; then
    print -- '[wm2000-operational-boundary] schema=fn64.wm2000.operational-boundary.v1 component_schema=fn64.operational-state-component-digests.v1 publication_schema=fn64.operational-thread-publication-digests.v2 capture_relation=latest_per_thread_publication_paired_with_post_scheduler_owner_snapshots device_sha256=dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd executor_sha256=eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee abi_host_sha256=ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff cpu_sha256=8888888888888888888888888888888888888888888888888888888888888888 continuation_sha256=9999999999999999999999999999999999999999999999999999999999999999 executor_threads=1 publications=1 exact=0 opaque=1 opaque_host=0 parked_fault=1 returned=0 missing=0 unexpected=0 cpu_comparable=false mutation_sealed=true pending_writes=0 open_host_transactions=0 mutation_quiescent=true'
elif [[ "${FN64_TEST_NONCOMPARABLE_CPU:-}" == 1 ]]; then
    print -- '[wm2000-operational-boundary] schema=fn64.wm2000.operational-boundary.v1 component_schema=fn64.operational-state-component-digests.v1 publication_schema=fn64.operational-thread-publication-digests.v2 capture_relation=latest_per_thread_publication_paired_with_post_scheduler_owner_snapshots device_sha256=dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd executor_sha256=eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee abi_host_sha256=ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff cpu_sha256=8888888888888888888888888888888888888888888888888888888888888888 continuation_sha256=9999999999999999999999999999999999999999999999999999999999999999 executor_threads=1 publications=1 exact=0 opaque=1 opaque_host=1 parked_fault=0 returned=0 missing=0 unexpected=0 cpu_comparable=false mutation_sealed=true pending_writes=0 open_host_transactions=0 mutation_quiescent=true'
else
    print -- '[wm2000-operational-boundary] schema=fn64.wm2000.operational-boundary.v1 component_schema=fn64.operational-state-component-digests.v1 publication_schema=fn64.operational-thread-publication-digests.v2 capture_relation=latest_per_thread_publication_paired_with_post_scheduler_owner_snapshots device_sha256=dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd executor_sha256=eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee abi_host_sha256=ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff cpu_sha256=8888888888888888888888888888888888888888888888888888888888888888 continuation_sha256=9999999999999999999999999999999999999999999999999999999999999999 executor_threads=1 publications=1 exact=1 opaque=0 opaque_host=0 parked_fault=0 returned=0 missing=0 unexpected=0 cpu_comparable=true mutation_sealed=true pending_writes=0 open_host_transactions=0 mutation_quiescent=true'
fi
if [[ "${FN64_TEST_MUTATE_INPUT:-}" == 1 ]]; then
    print -n -- x >>"$FN64_CONTROLLER_SCHEDULE"
fi
EOF
cat >"$test_dynamic" <<'EOF'
#!/bin/zsh
[[ "$FN64_BLOCK_MIN_GUEST_INSTRUCTIONS" == 1000004 ]]
[[ "$FN64_BLOCK_EXPECT_GUEST_INSTRUCTIONS" == 1000004 ]]
[[ "$FN64_DYNAMIC_WITHHOLD_CANONICAL_ENTRY" == 1 ]]
rom_digest_line=$(shasum -a 256 -- "$ROM")
rom_sha=${rom_digest_line%% *}
if [[ "${FN64_TEST_STALE_TELEMETRY:-}" == 1 ]]; then
    rom_sha=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
fi
program_sha=5555555555555555555555555555555555555555555555555555555555555555
if [[ "${FN64_TEST_IDENTITY_DRIFT:-}" == 1 ]]; then
    program_sha=9999999999999999999999999999999999999999999999999999999999999999
fi
resolver_sha=1111111111111111111111111111111111111111111111111111111111111111
if [[ "${FN64_TEST_RESOLVER_DRIFT:-}" == 1 ]]; then
    resolver_sha=9999999999999999999999999999999999999999999999999999999999999999
fi
print -- "[wm2000-program-identity] schema=fn64.wm2000.program-identity.v1 sha256=$program_sha source=canonical_block_program_sha256 resolver_sha256=$resolver_sha entry_bank=0000000000000001 entry_pc=80001000"
telemetry_schema=fn64.wm2000.dynamic-withheld-telemetry.v2
if [[ "${FN64_TEST_V1_TELEMETRY:-}" == 1 ]]; then
    telemetry_schema=fn64.wm2000.dynamic-withheld-telemetry.v1
fi
withheld_pc=80001000
if [[ "${FN64_TEST_WRONG_WITHHELD_PC:-}" == 1 ]]; then
    withheld_pc=80001004
fi
charged_instructions=4
attempted_charged_instructions=4
if [[ "${FN64_TEST_ZERO_DYNAMIC_WORK:-}" == 1 ]]; then
    attempted_charged_instructions=0
fi
device_sha=dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd
if [[ "${FN64_TEST_COMPONENT_MISMATCH:-}" == 1 ]]; then
    device_sha=cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc
fi
exact_count=1
opaque_count=0
opaque_host_count=0
parked_fault_count=0
cpu_comparable=true
cpu_sha=8888888888888888888888888888888888888888888888888888888888888888
continuation_sha=9999999999999999999999999999999999999999999999999999999999999999
if [[ "${FN64_TEST_CPU_MISMATCH:-}" == 1 ]]; then
    cpu_sha=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
fi
if [[ "${FN64_TEST_CONTINUATION_MISMATCH:-}" == 1 ]]; then
    continuation_sha=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
fi
if [[ "${FN64_TEST_NONCOMPARABLE_CPU:-}" == 1 ]]; then
    exact_count=0
    opaque_count=1
    opaque_host_count=1
    cpu_comparable=false
fi
if [[ "${FN64_TEST_PARKED_FAULT:-}" == 1 ]]; then
    exact_count=0
    opaque_count=1
    opaque_host_count=0
    parked_fault_count=1
    cpu_comparable=false
fi
cat >"$FN64_DYNAMIC_TELEMETRY" <<'JSON'
{
  "schema": "TELEMETRY_SCHEMA_PLACEHOLDER",
  "authority": "operational_only_dynamic_installed",
  "claim": "dynamically_executed_exact_withheld_static_key",
  "selection_basis": "validated_canonical_catalog_entry",
  "resolver_install_sha256": "1111111111111111111111111111111111111111111111111111111111111111",
  "dynamic_source_sha256": "2222222222222222222222222222222222222222222222222222222222222222",
  "rom_sha256": "ROM_SHA_PLACEHOLDER",
  "bootstrap_receipt_sha256": "3333333333333333333333333333333333333333333333333333333333333333",
  "mutation_journal_root_sha256": "4444444444444444444444444444444444444444444444444444444444444444",
  "program_identity": {
    "sha256": "5555555555555555555555555555555555555555555555555555555555555555",
    "source": "canonical_block_program_sha256"
  },
  "withheld": {
    "bank": "0000000000000001",
    "pc": "WITHHELD_PC_PLACEHOLDER"
  },
  "guest_instruction_horizon": {
    "minimum": 1000004,
    "expected_exact": 1000004,
    "achieved": 1000004,
    "expected_match": true
  },
  "full_logical_rdram": {
    "byte_len": 8388608,
    "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
  },
  "dropped_identity_activations": 0,
  "dropped_identity_charged_instructions": 0,
  "dropped_identity_unsupported_exits": 0,
  "dropped_attempted_entry_activations": 0,
  "dropped_attempted_entry_charged_instructions": 0,
  "dropped_attempted_entry_unsupported_exits": 0,
  "mutation_quiescence": {
    "mutation_sealed": true,
    "pending_attributed_writes": 0,
    "open_host_transactions": 0,
    "mutation_journal_quiescent": true
  },
  "operational_boundary": {
    "schema": "fn64.wm2000.operational-boundary.v1",
    "component_schema": "fn64.operational-state-component-digests.v1",
    "publication_schema": "fn64.operational-thread-publication-digests.v2",
    "capture_relation": "latest_per_thread_publication_paired_with_post_scheduler_owner_snapshots",
    "components": {
      "device_sha256": "DEVICE_SHA_PLACEHOLDER",
      "executor_sha256": "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
      "abi_host_sha256": "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
    },
    "thread_publications": {
      "cpu_sha256": "CPU_SHA_PLACEHOLDER",
      "continuation_sha256": "CONTINUATION_SHA_PLACEHOLDER",
      "executor_threads": 1,
      "publication_count": 1,
      "exact_count": EXACT_COUNT_PLACEHOLDER,
      "opaque_count": OPAQUE_COUNT_PLACEHOLDER,
      "opaque_host_count": OPAQUE_HOST_COUNT_PLACEHOLDER,
      "parked_fault_count": PARKED_FAULT_COUNT_PLACEHOLDER,
      "returned_count": 0,
      "missing_count": 0,
      "unexpected_count": 0,
      "cpu_comparable": CPU_COMPARABLE_PLACEHOLDER
    },
    "mutation_quiescence": {
      "mutation_sealed": true,
      "pending_attributed_writes": 0,
      "open_host_transactions": 0,
      "mutation_journal_quiescent": true
    }
  },
  "aggregates": [{
    "admitted_bank": "0000000000000001",
    "admitted_pc": "80001000",
    "attempted_entries": [{
      "bank": "0000000000000001",
      "pc": "80001000",
      "activations": 1,
      "charged_instructions": ATTEMPTED_CHARGED_INSTRUCTIONS_PLACEHOLDER,
      "unsupported_exits": 0
    }, {
      "bank": "0000000000000001",
      "pc": "80001004",
      "activations": 1,
      "charged_instructions": 4,
      "unsupported_exits": 0
    }],
    "charged_instructions": CHARGED_INSTRUCTIONS_PLACEHOLDER,
    "unsupported_exits": 0
  }],
  "termination": {
    "mode": "bounded_progress_only",
    "stop_cause": "minimum_guest_instruction_checkpoint_reached"
  },
  "scheduler_boundary": {
    "steps": 42,
    "sim_time": 9001
  }
}
JSON
sed -i '' "s/ROM_SHA_PLACEHOLDER/$rom_sha/" "$FN64_DYNAMIC_TELEMETRY"
sed -i '' "s/TELEMETRY_SCHEMA_PLACEHOLDER/$telemetry_schema/" "$FN64_DYNAMIC_TELEMETRY"
sed -i '' "s/WITHHELD_PC_PLACEHOLDER/$withheld_pc/" "$FN64_DYNAMIC_TELEMETRY"
sed -i '' "s/ATTEMPTED_CHARGED_INSTRUCTIONS_PLACEHOLDER/$attempted_charged_instructions/" "$FN64_DYNAMIC_TELEMETRY"
sed -i '' "s/CHARGED_INSTRUCTIONS_PLACEHOLDER/$charged_instructions/" "$FN64_DYNAMIC_TELEMETRY"
sed -i '' "s/DEVICE_SHA_PLACEHOLDER/$device_sha/" "$FN64_DYNAMIC_TELEMETRY"
sed -i '' "s/CPU_SHA_PLACEHOLDER/$cpu_sha/" "$FN64_DYNAMIC_TELEMETRY"
sed -i '' "s/CONTINUATION_SHA_PLACEHOLDER/$continuation_sha/" "$FN64_DYNAMIC_TELEMETRY"
sed -i '' "s/EXACT_COUNT_PLACEHOLDER/$exact_count/" "$FN64_DYNAMIC_TELEMETRY"
sed -i '' "s/OPAQUE_COUNT_PLACEHOLDER/$opaque_count/" "$FN64_DYNAMIC_TELEMETRY"
sed -i '' "s/OPAQUE_HOST_COUNT_PLACEHOLDER/$opaque_host_count/" "$FN64_DYNAMIC_TELEMETRY"
sed -i '' "s/PARKED_FAULT_COUNT_PLACEHOLDER/$parked_fault_count/" "$FN64_DYNAMIC_TELEMETRY"
sed -i '' "s/CPU_COMPARABLE_PLACEHOLDER/$cpu_comparable/" "$FN64_DYNAMIC_TELEMETRY"
EOF
chmod +x "$test_aot" "$test_dynamic"
typeset test_rom_digest_line test_boot_digest_line test_aot_digest_line test_dynamic_digest_line
test_rom_digest_line=$(shasum -a 256 -- "$test_rom")
test_boot_digest_line=$(shasum -a 256 -- "$test_boot")
test_aot_digest_line=$(shasum -a 256 -- "$test_aot")
test_dynamic_digest_line=$(shasum -a 256 -- "$test_dynamic")
typeset -r test_rom_sha=${test_rom_digest_line%% *}
typeset -r test_boot_sha=${test_boot_digest_line%% *}
typeset -r test_aot_sha=${test_aot_digest_line%% *}
typeset -r test_dynamic_sha=${test_dynamic_digest_line%% *}
cat >"$test_receipt" <<EOF
{
  "schema": "fn64.wm2000.withheld-pair-build-receipt.v4",
  "authority": "build_local_non_authoritative",
  "evidence_scope": "operational_build_local_artifact_identity_only",
  "privacy": "path-free-private-input-identities-only",
  "identity_relation": "pre_use_hashes_verified_unchanged_after_both_builds",
  "inputs": {
    "raw_rom_sha256": "$test_rom_sha",
    "boot_context_sha256": "$test_boot_sha",
    "executable_image_capture_groups": [{
      "name": "FN64_EXECUTABLE_IMAGES",
      "ordered_capture_sha256": [
        "1111111111111111111111111111111111111111111111111111111111111111",
        "2222222222222222222222222222222222222222222222222222222222222222",
        "3333333333333333333333333333333333333333333333333333333333333333"
      ]
    }],
    "manifest_sha256": "4444444444444444444444444444444444444444444444444444444444444444",
    "lock_sha256": "5555555555555555555555555555555555555555555555555555555555555555"
  },
  "artifacts": {
    "aot_sha256": "$test_aot_sha",
    "dynamic_withheld_sha256": "$test_dynamic_sha",
    "aot_feature_tree_log_sha256": "6666666666666666666666666666666666666666666666666666666666666666",
    "pure_aot_checker_log_sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    "dynamic_feature_tree_log_sha256": "7777777777777777777777777777777777777777777777777777777777777777",
    "target_subdir": "cargo-target",
    "cargo_cache_seed": "none"
  },
  "guard": {
    "max_rss_mib": 4096,
    "min_free_percent": 40,
    "max_seconds": 3600,
    "poll_seconds": "1",
    "cargo_build_jobs": 1,
    "memory_guard_sha256": "8888888888888888888888888888888888888888888888888888888888888888",
    "pure_aot_checker_sha256": "9999999999999999999999999999999999999999999999999999999999999999",
    "cargo_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "aot_feature_check_jsonl": "aot-feature-check-memory-guard.jsonl",
    "dynamic_feature_check_jsonl": "dynamic-feature-check-memory-guard.jsonl",
    "aot_jsonl": "aot-memory-guard.jsonl",
    "dynamic_withheld_jsonl": "dynamic-withheld-memory-guard.jsonl",
    "aot_feature_check_measurement": {"samples": 1, "elapsed_seconds": 1, "peak_tree_rss_mib": 1, "last_reason": "complete"},
    "dynamic_feature_check_measurement": {"samples": 1, "elapsed_seconds": 1, "peak_tree_rss_mib": 1, "last_reason": "complete"},
    "aot_measurement": {"samples": 1, "elapsed_seconds": 1, "peak_tree_rss_mib": 1, "last_reason": "complete"},
    "dynamic_withheld_measurement": {"samples": 1, "elapsed_seconds": 1, "peak_tree_rss_mib": 1, "last_reason": "complete"}
  },
  "commands": {
    "pure_aot_feature_gate": ["cargo", "tree", "-e", "normal,features"],
    "pure_aot_checker_gate": ["scripts/check-wm2000-pure-aot.zsh"],
    "dynamic_feature_gate": ["cargo", "tree", "--features", "dynamic-withheld"],
    "aot": ["cargo", "build", "-p", "wm2000-block-boot"],
    "dynamic_withheld": ["cargo", "build", "-p", "wm2000-block-boot", "--features", "dynamic-withheld"]
  }
}
EOF
python3 - "$test_receipt" "$test_v3_receipt" <<'PY'
import json
import sys

source, destination = sys.argv[1:]
receipt = json.load(open(source, encoding="utf-8"))
receipt["schema"] = "fn64.wm2000.withheld-pair-build-receipt.v3"
receipt["artifacts"]["dynamic_source_check_log_sha256"] = "c" * 64
receipt["guard"]["dynamic_source_check_jsonl"] = "dynamic-source-check-memory-guard.jsonl"
receipt["guard"]["dynamic_source_check_measurement"] = {
    "samples": 1,
    "elapsed_seconds": 1,
    "peak_tree_rss_mib": 1,
    "last_reason": "complete",
}
receipt["commands"]["dynamic_source_check"] = [
    "cargo", "check", "--bin", "wm2000-block-boot", "--features", "dynamic-withheld",
]
with open(destination, "x", encoding="utf-8") as output:
    json.dump(receipt, output, sort_keys=True, indent=2)
    output.write("\n")
PY
cp "$test_v3_receipt" "$test_mixed_v4_receipt"
sed -i '' 's/withheld-pair-build-receipt\.v3/withheld-pair-build-receipt.v4/' "$test_mixed_v4_receipt"
cp "$test_receipt" "$test_stale_receipt"
sed -i '' "s/$test_rom_sha/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/" "$test_stale_receipt"
cp "$test_receipt" "$test_unpaired_receipt"
sed -i '' "s/$test_aot_sha/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb/" "$test_unpaired_receipt"

if ! ROM=$test_rom \
    FN64_BOOT_CONTEXT=$test_boot \
    FN64_WM_PAIR_RECEIPT=$test_receipt \
    FN64_WM_AOT_BINARY=$test_aot \
    FN64_WM_DYNAMIC_BINARY=$test_dynamic \
    FN64_WM_DIFF_OUTPUT_DIR=$test_output \
    "$test_root/scripts/wm2000-withheld-rdram-diff.zsh" "$test_schedule" 1000000 2000000 \
        >"$test_dir/wrapper.log" 2>&1
then
    print -u2 -- "test-wm2000-withheld-rdram-diff: positive wrapper invocation failed"
    for diagnostic in "$test_dir/wrapper.log" "$test_output/aot.log" "$test_output/dynamic.log"; do
        if [[ -f "$diagnostic" ]]; then
            print -u2 -- "--- ${diagnostic:t} ---"
            command cat -- "$diagnostic" >&2
        fi
    done
    exit 1
fi

if ! ROM=$test_rom \
    FN64_BOOT_CONTEXT=$test_boot \
    FN64_WM_PAIR_RECEIPT=$test_v3_receipt \
    FN64_WM_AOT_BINARY=$test_aot \
    FN64_WM_DYNAMIC_BINARY=$test_dynamic \
    FN64_WM_DIFF_OUTPUT_DIR=$test_v3_output \
    "$test_root/scripts/wm2000-withheld-rdram-diff.zsh" "$test_schedule" 1000000 2000000 \
        >"$test_dir/v3-wrapper.log" 2>&1
then
    print -u2 -- "test-wm2000-withheld-rdram-diff: retained v3 receipt was rejected"
    command cat -- "$test_dir/v3-wrapper.log" >&2
    exit 1
fi

rg -q '^wm2000 withheld diff: MATCH guest_instructions=1000004 ' "$test_dir/wrapper.log"
rg -q '"receipt_schema": "fn64.wm2000.withheld-pair-build-receipt.v4"' "$test_output/comparison.json"
rg -q '"receipt_schema": "fn64.wm2000.withheld-pair-build-receipt.v3"' "$test_v3_output/comparison.json"
if ROM=$test_rom \
    FN64_BOOT_CONTEXT=$test_boot \
    FN64_WM_PAIR_RECEIPT=$test_mixed_v4_receipt \
    FN64_WM_AOT_BINARY=$test_aot \
    FN64_WM_DYNAMIC_BINARY=$test_dynamic \
    FN64_WM_DIFF_OUTPUT_DIR=$test_mixed_v4_output \
    "$test_root/scripts/wm2000-withheld-rdram-diff.zsh" "$test_schedule" 1000000 2000000 \
        >"$test_dir/mixed-v4.log" 2>&1
then
    print -u2 -- "test-wm2000-withheld-rdram-diff: v4 receipt accepted v3-only fields"
    exit 1
fi
rg -q 'invalid pair receipt: artifacts fields' "$test_dir/mixed-v4.log"
[[ ! -e "$test_mixed_v4_output/aot.log" ]]
rg -q '"match": true' "$test_output/comparison.json"
rg -q '"authority": "operational_rdram_and_owner_components_with_published_cpu_diagnostics"' "$test_output/comparison.json"
rg -q '"device_match": true' "$test_output/comparison.json"
rg -q '"executor_match": true' "$test_output/comparison.json"
rg -q '"abi_host_match": true' "$test_output/comparison.json"
rg -q '"cpu_comparable": true' "$test_output/comparison.json"
rg -q '"cpu_match": true' "$test_output/comparison.json"
rg -q '"continuation_match": true' "$test_output/comparison.json"
rg -q '"partial_owner_match": true' "$test_output/comparison.json"
rg -q '"published_cpu_gate_pass": true' "$test_output/comparison.json"
rg -q '"operational_match": true' "$test_output/comparison.json"
rg -q '"scheduler_steps_match": true' "$test_output/comparison.json"
rg -q '"sim_time_match": true' "$test_output/comparison.json"
if ROM=$test_rom \
    FN64_BOOT_CONTEXT=$test_boot \
    FN64_WM_PAIR_RECEIPT=$test_receipt \
    FN64_WM_AOT_BINARY=$test_aot \
    FN64_WM_DYNAMIC_BINARY=$test_dynamic \
    FN64_WM_DIFF_OUTPUT_DIR=$test_noncomparable_output \
    FN64_TEST_NONCOMPARABLE_CPU=1 \
    "$test_root/scripts/wm2000-withheld-rdram-diff.zsh" "$test_schedule" 1000000 2000000 \
    >"$test_dir/noncomparable.log" 2>&1
then
    print -u2 -- "test-wm2000-withheld-rdram-diff: non-comparable CPU publication produced a match"
    exit 1
fi
rg -q 'operational comparison unproven: CPU publications are not comparable' "$test_dir/noncomparable.log"
rg -q '"match": null' "$test_noncomparable_output/comparison.json"
rg -q '"partial_owner_match": true' "$test_noncomparable_output/comparison.json"
rg -q '"operational_match": null' "$test_noncomparable_output/comparison.json"
rg -q '"published_cpu_gate_pass": null' "$test_noncomparable_output/comparison.json"
rg -q '"cpu_comparable": false' "$test_noncomparable_output/comparison.json"
rg -q '"cpu_match": null' "$test_noncomparable_output/comparison.json"
rg -q '"continuation_match": null' "$test_noncomparable_output/comparison.json"
if ROM=$test_rom \
    FN64_BOOT_CONTEXT=$test_boot \
    FN64_WM_PAIR_RECEIPT=$test_receipt \
    FN64_WM_AOT_BINARY=$test_aot \
    FN64_WM_DYNAMIC_BINARY=$test_dynamic \
    FN64_WM_DIFF_OUTPUT_DIR=$test_parked_fault_output \
    FN64_TEST_PARKED_FAULT=1 \
    "$test_root/scripts/wm2000-withheld-rdram-diff.zsh" "$test_schedule" 1000000 2000000 \
    >"$test_dir/parked-fault.log" 2>&1
then
    print -u2 -- "test-wm2000-withheld-rdram-diff: parked-fault publication produced a match"
    exit 1
fi
rg -q 'operational comparison unproven: CPU publications are not comparable' "$test_dir/parked-fault.log"
rg -q '"match": null' "$test_parked_fault_output/comparison.json"
python3 - "$test_parked_fault_output/comparison.json" <<'PY'
import json
import sys

comparison = json.load(open(sys.argv[1], encoding="utf-8"))
expected = [1, 1, 0, 1, 0, 1, 0, 0, 0]
assert comparison["aot_publication_profile"] == expected
assert comparison["dynamic_publication_profile"] == expected
assert comparison["cpu_comparable"] is False
PY
if rg -q '^wm2000 withheld diff: MATCH ' "$test_dir/parked-fault.log"; then
    print -u2 -- "test-wm2000-withheld-rdram-diff: parked-fault failure printed MATCH"
    exit 1
fi
if ROM=$test_rom \
    FN64_BOOT_CONTEXT=$test_boot \
    FN64_WM_PAIR_RECEIPT=$test_receipt \
    FN64_WM_AOT_BINARY=$test_aot \
    FN64_WM_DYNAMIC_BINARY=$test_dynamic \
    FN64_WM_DIFF_OUTPUT_DIR=$test_cpu_mismatch_output \
    FN64_TEST_CPU_MISMATCH=1 \
    "$test_root/scripts/wm2000-withheld-rdram-diff.zsh" "$test_schedule" 1000000 2000000 \
    >"$test_dir/cpu-mismatch.log" 2>&1
then
    print -u2 -- "test-wm2000-withheld-rdram-diff: comparable CPU mismatch was accepted"
    exit 1
fi
rg -q 'operational comparison mismatch' "$test_dir/cpu-mismatch.log"
rg -q '"partial_owner_match": true' "$test_cpu_mismatch_output/comparison.json"
rg -q '"cpu_comparable": true' "$test_cpu_mismatch_output/comparison.json"
rg -q '"cpu_match": false' "$test_cpu_mismatch_output/comparison.json"
rg -q '"continuation_match": true' "$test_cpu_mismatch_output/comparison.json"
rg -q '"published_cpu_gate_pass": false' "$test_cpu_mismatch_output/comparison.json"
rg -q '"operational_match": false' "$test_cpu_mismatch_output/comparison.json"
rg -q '"match": false' "$test_cpu_mismatch_output/comparison.json"
if ROM=$test_rom \
    FN64_BOOT_CONTEXT=$test_boot \
    FN64_WM_PAIR_RECEIPT=$test_receipt \
    FN64_WM_AOT_BINARY=$test_aot \
    FN64_WM_DYNAMIC_BINARY=$test_dynamic \
    FN64_WM_DIFF_OUTPUT_DIR=$test_continuation_mismatch_output \
    FN64_TEST_CONTINUATION_MISMATCH=1 \
    "$test_root/scripts/wm2000-withheld-rdram-diff.zsh" "$test_schedule" 1000000 2000000 \
    >"$test_dir/continuation-mismatch.log" 2>&1
then
    print -u2 -- "test-wm2000-withheld-rdram-diff: comparable continuation mismatch was accepted"
    exit 1
fi
rg -q 'operational comparison mismatch' "$test_dir/continuation-mismatch.log"
rg -q '"partial_owner_match": true' "$test_continuation_mismatch_output/comparison.json"
rg -q '"cpu_comparable": true' "$test_continuation_mismatch_output/comparison.json"
rg -q '"cpu_match": true' "$test_continuation_mismatch_output/comparison.json"
rg -q '"continuation_match": false' "$test_continuation_mismatch_output/comparison.json"
rg -q '"published_cpu_gate_pass": false' "$test_continuation_mismatch_output/comparison.json"
rg -q '"operational_match": false' "$test_continuation_mismatch_output/comparison.json"
rg -q '"match": false' "$test_continuation_mismatch_output/comparison.json"
if ROM=$test_rom \
    FN64_BOOT_CONTEXT=$test_boot \
    FN64_WM_PAIR_RECEIPT=$test_receipt \
    FN64_WM_AOT_BINARY=$test_aot \
    FN64_WM_DYNAMIC_BINARY=$test_dynamic \
    FN64_WM_DIFF_OUTPUT_DIR=$test_stale_output \
    FN64_TEST_STALE_TELEMETRY=1 \
    "$test_root/scripts/wm2000-withheld-rdram-diff.zsh" "$test_schedule" 1000000 2000000 \
    >"$test_dir/stale-telemetry.log" 2>&1
then
    print -u2 -- "test-wm2000-withheld-rdram-diff: stale telemetry was accepted"
    exit 1
fi
rg -q 'invalid dynamic telemetry: ROM identity' "$test_dir/stale-telemetry.log"
if ROM=$test_rom \
    FN64_BOOT_CONTEXT=$test_boot \
    FN64_WM_PAIR_RECEIPT=$test_receipt \
    FN64_WM_AOT_BINARY=$test_aot \
    FN64_WM_DYNAMIC_BINARY=$test_dynamic \
    FN64_WM_DIFF_OUTPUT_DIR=$test_wrong_pc_output \
    FN64_TEST_WRONG_WITHHELD_PC=1 \
    "$test_root/scripts/wm2000-withheld-rdram-diff.zsh" "$test_schedule" 1000000 2000000 \
    >"$test_dir/wrong-pc.log" 2>&1
then
    print -u2 -- "test-wm2000-withheld-rdram-diff: wrong withheld PC was accepted"
    exit 1
fi
rg -q 'invalid dynamic telemetry: withheld key equals canonical program entry' "$test_dir/wrong-pc.log"
if ROM=$test_rom \
    FN64_BOOT_CONTEXT=$test_boot \
    FN64_WM_PAIR_RECEIPT=$test_receipt \
    FN64_WM_AOT_BINARY=$test_aot \
    FN64_WM_DYNAMIC_BINARY=$test_dynamic \
    FN64_WM_DIFF_OUTPUT_DIR=$test_identity_drift_output \
    FN64_TEST_IDENTITY_DRIFT=1 \
    "$test_root/scripts/wm2000-withheld-rdram-diff.zsh" "$test_schedule" 1000000 2000000 \
    >"$test_dir/identity-drift.log" 2>&1
then
    print -u2 -- "test-wm2000-withheld-rdram-diff: lane program-identity drift was accepted"
    exit 1
fi
rg -q 'AOT/dynamic program identity drift' "$test_dir/identity-drift.log"
if ROM=$test_rom \
    FN64_BOOT_CONTEXT=$test_boot \
    FN64_WM_PAIR_RECEIPT=$test_receipt \
    FN64_WM_AOT_BINARY=$test_aot \
    FN64_WM_DYNAMIC_BINARY=$test_dynamic \
    FN64_WM_DIFF_OUTPUT_DIR=$test_resolver_drift_output \
    FN64_TEST_RESOLVER_DRIFT=1 \
    "$test_root/scripts/wm2000-withheld-rdram-diff.zsh" "$test_schedule" 1000000 2000000 \
    >"$test_dir/resolver-drift.log" 2>&1
then
    print -u2 -- "test-wm2000-withheld-rdram-diff: lane resolver drift was accepted"
    exit 1
fi
rg -q 'AOT/dynamic program identity drift' "$test_dir/resolver-drift.log"
if ROM=$test_rom \
    FN64_BOOT_CONTEXT=$test_boot \
    FN64_WM_PAIR_RECEIPT=$test_receipt \
    FN64_WM_AOT_BINARY=$test_aot \
    FN64_WM_DYNAMIC_BINARY=$test_dynamic \
    FN64_WM_DIFF_OUTPUT_DIR=$test_zero_dynamic_work_output \
    FN64_TEST_ZERO_DYNAMIC_WORK=1 \
    "$test_root/scripts/wm2000-withheld-rdram-diff.zsh" "$test_schedule" 1000000 2000000 \
    >"$test_dir/zero-dynamic-work.log" 2>&1
then
    print -u2 -- "test-wm2000-withheld-rdram-diff: zero charged dynamic work was accepted"
    exit 1
fi
rg -q 'invalid dynamic telemetry: charged dynamic execution of exact withheld canonical entry' "$test_dir/zero-dynamic-work.log"
if ROM=$test_rom \
    FN64_BOOT_CONTEXT=$test_boot \
    FN64_WM_PAIR_RECEIPT=$test_receipt \
    FN64_WM_AOT_BINARY=$test_aot \
    FN64_WM_DYNAMIC_BINARY=$test_dynamic \
    FN64_WM_DIFF_OUTPUT_DIR=$test_v1_telemetry_output \
    FN64_TEST_V1_TELEMETRY=1 \
    "$test_root/scripts/wm2000-withheld-rdram-diff.zsh" "$test_schedule" 1000000 2000000 \
    >"$test_dir/v1-telemetry.log" 2>&1
then
    print -u2 -- "test-wm2000-withheld-rdram-diff: stale v1 telemetry was accepted"
    exit 1
fi
rg -q 'invalid dynamic telemetry: schema' "$test_dir/v1-telemetry.log"
if ROM=$test_rom \
    FN64_BOOT_CONTEXT=$test_boot \
    FN64_WM_PAIR_RECEIPT=$test_receipt \
    FN64_WM_AOT_BINARY=$test_aot \
    FN64_WM_DYNAMIC_BINARY=$test_dynamic \
    FN64_WM_DIFF_OUTPUT_DIR=$test_component_output \
    FN64_TEST_COMPONENT_MISMATCH=1 \
    "$test_root/scripts/wm2000-withheld-rdram-diff.zsh" "$test_schedule" 1000000 2000000 \
    >"$test_dir/component-mismatch.log" 2>&1
then
    print -u2 -- "test-wm2000-withheld-rdram-diff: component mismatch was accepted"
    exit 1
fi
rg -q 'operational comparison failed' "$test_dir/component-mismatch.log"
rg -q '"device_match": false' "$test_component_output/comparison.json"
if ROM=$test_rom \
    FN64_BOOT_CONTEXT=$test_boot \
    FN64_WM_PAIR_RECEIPT=$test_stale_receipt \
    FN64_WM_AOT_BINARY=$test_aot \
    FN64_WM_DYNAMIC_BINARY=$test_dynamic \
    FN64_WM_DIFF_OUTPUT_DIR=$test_stale_receipt_output \
    "$test_root/scripts/wm2000-withheld-rdram-diff.zsh" "$test_schedule" 1000000 2000000 \
    >"$test_dir/stale-receipt.log" 2>&1
then
    print -u2 -- "test-wm2000-withheld-rdram-diff: stale ROM receipt was accepted"
    exit 1
fi
rg -q 'invalid pair receipt: raw ROM identity' "$test_dir/stale-receipt.log"
[[ ! -e "$test_stale_receipt_output/aot.log" ]]
if ROM=$test_rom \
    FN64_BOOT_CONTEXT=$test_boot \
    FN64_WM_PAIR_RECEIPT=$test_unpaired_receipt \
    FN64_WM_AOT_BINARY=$test_aot \
    FN64_WM_DYNAMIC_BINARY=$test_dynamic \
    FN64_WM_DIFF_OUTPUT_DIR=$test_unpaired_receipt_output \
    "$test_root/scripts/wm2000-withheld-rdram-diff.zsh" "$test_schedule" 1000000 2000000 \
    >"$test_dir/unpaired-receipt.log" 2>&1
then
    print -u2 -- "test-wm2000-withheld-rdram-diff: unpaired AOT executable was accepted"
    exit 1
fi
rg -q 'invalid pair receipt: AOT executable identity' "$test_dir/unpaired-receipt.log"
[[ ! -e "$test_unpaired_receipt_output/aot.log" ]]
if ROM=$test_rom \
    FN64_BOOT_CONTEXT=$test_boot \
    FN64_WM_PAIR_RECEIPT=$test_receipt \
    FN64_WM_AOT_BINARY=$test_aot \
    FN64_WM_DYNAMIC_BINARY=$test_dynamic \
    FN64_WM_DIFF_OUTPUT_DIR=$test_mutation_output \
    FN64_TEST_MUTATE_INPUT=1 \
    "$test_root/scripts/wm2000-withheld-rdram-diff.zsh" "$test_schedule" 1000000 2000000 \
    >"$test_dir/input-mutation.log" 2>&1
then
    print -u2 -- "test-wm2000-withheld-rdram-diff: controller-schedule mutation was accepted"
    exit 1
fi
rg -q 'controller-schedule changed during comparison; refusing evidence' "$test_dir/input-mutation.log"
if ROM=$test_rom \
    FN64_BOOT_CONTEXT=$test_boot \
    FN64_WM_PAIR_RECEIPT=$test_receipt \
    FN64_WM_AOT_BINARY=$test_aot \
    FN64_WM_DYNAMIC_BINARY=$test_dynamic \
    FN64_WM_DIFF_OUTPUT_DIR=$test_root \
    "$test_root/scripts/wm2000-withheld-rdram-diff.zsh" "$test_schedule" \
    >"$test_dir/repository-output.log" 2>&1
then
    print -u2 -- "test-wm2000-withheld-rdram-diff: repository output was accepted"
    exit 1
fi
rg -q 'output artifacts must remain outside the repository' "$test_dir/repository-output.log"
for negative_log in \
    "$test_dir/noncomparable.log" \
    "$test_dir/cpu-mismatch.log" \
    "$test_dir/continuation-mismatch.log" \
    "$test_dir/stale-telemetry.log" \
    "$test_dir/wrong-pc.log" \
    "$test_dir/identity-drift.log" \
    "$test_dir/resolver-drift.log" \
    "$test_dir/zero-dynamic-work.log" \
    "$test_dir/v1-telemetry.log" \
    "$test_dir/component-mismatch.log" \
    "$test_dir/stale-receipt.log" \
    "$test_dir/unpaired-receipt.log" \
    "$test_dir/input-mutation.log" \
    "$test_dir/repository-output.log"
do
    if rg -q '^wm2000 withheld diff: MATCH ' "$negative_log"; then
        print -u2 -- "test-wm2000-withheld-rdram-diff: negative fixture printed MATCH: ${negative_log:t}"
        exit 1
    fi
done
print -- "test-wm2000-withheld-rdram-diff: pass"
