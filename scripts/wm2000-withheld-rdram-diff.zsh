#!/bin/zsh

# Compare a retained pure-AOT WM2000 binary with a separately built
# dynamic-withheld binary at the same global canonical guest-work count.
# This is a full-RDRAM plus canonical owner-component operational
# differential. Owner-component equality remains useful partial evidence when
# either lane has opaque thread state, but only comparable, equal publication
# digests can produce an operational match. This is not an atomic
# full-machine-state proof.

set -eu
setopt NOCLOBBER

if (( $# < 1 || $# > 3 )); then
    print -u2 -- "usage: ROM=... FN64_BOOT_CONTEXT=... FN64_WM_PAIR_RECEIPT=... FN64_WM_AOT_BINARY=... FN64_WM_DYNAMIC_BINARY=... $0 SCHEDULE [MIN_GUEST_INSTRUCTIONS] [MAX_STEPS]"
    exit 2
fi
if [[ -z "${ROM:-}" || ! -f "$ROM" ]]; then
    print -u2 -- "wm2000 withheld diff: ROM must name the user's readable ROM"
    exit 2
fi
if [[ -z "${FN64_BOOT_CONTEXT:-}" || ! -f "$FN64_BOOT_CONTEXT" ]]; then
    print -u2 -- "wm2000 withheld diff: FN64_BOOT_CONTEXT must name a readable capture"
    exit 2
fi
if [[ -z "${FN64_WM_PAIR_RECEIPT:-}" || ! -f "$FN64_WM_PAIR_RECEIPT" || -L "$FN64_WM_PAIR_RECEIPT" ]]; then
    print -u2 -- "wm2000 withheld diff: FN64_WM_PAIR_RECEIPT must name a readable non-symlink pair-build receipt"
    exit 2
fi
if [[ -z "${FN64_WM_AOT_BINARY:-}" || ! -x "$FN64_WM_AOT_BINARY" ]]; then
    print -u2 -- "wm2000 withheld diff: FN64_WM_AOT_BINARY must name a retained executable"
    exit 2
fi
if [[ -z "${FN64_WM_DYNAMIC_BINARY:-}" || ! -x "$FN64_WM_DYNAMIC_BINARY" ]]; then
    print -u2 -- "wm2000 withheld diff: FN64_WM_DYNAMIC_BINARY must name a separately built dynamic-withheld executable"
    exit 2
fi

typeset -r diff_root=${0:A:h:h}
typeset -r diff_schedule=${1:A}
typeset -r diff_minimum=${2:-1000000}
typeset -r diff_max_steps=${3:-2000000}
typeset -r diff_guard_max_rss_mib=${FN64_GUARD_MAX_RSS_MIB:-2048}
typeset -r diff_output_root=${FN64_WM_DIFF_OUTPUT_DIR:-$(mktemp -d /private/tmp/fn64-wm-withheld-diff.XXXXXX)}
typeset -r diff_aot_log=$diff_output_root/aot.log
typeset -r diff_dynamic_log=$diff_output_root/dynamic.log
typeset -r diff_dynamic_telemetry=$diff_output_root/dynamic-telemetry.json
typeset -r diff_comparison=$diff_output_root/comparison.json
typeset -a diff_common_environment

hash_file() {
    local digest_line
    digest_line=$(shasum -a 256 -- "$1") || {
        print -u2 -- "wm2000 withheld diff: cannot hash required input"
        exit 1
    }
    if [[ ! "$digest_line" =~ '^[0-9a-f]{64} ' ]]; then
        print -u2 -- "wm2000 withheld diff: hash tool returned a malformed SHA-256"
        exit 1
    fi
    print -r -- "${digest_line%% *}"
}

normalized_rom_hash() {
    python3 - "$1" <<'PY'
import hashlib
import sys

source = open(sys.argv[1], "rb")
header = source.read(4)
if header == bytes.fromhex("80371240"):
    unit = 1
elif header == bytes.fromhex("37804012"):
    unit = 2
elif header == bytes.fromhex("40123780"):
    unit = 4
else:
    raise SystemExit("wm2000 withheld diff: ROM has an unrecognized N64 byte-order header")
source.seek(0)
digest = hashlib.sha256()
while chunk := source.read(1024 * 1024):
    if len(chunk) % unit:
        raise SystemExit("wm2000 withheld diff: malformed byte-swapped ROM byte length")
    if unit == 2:
        normalized = bytearray(len(chunk))
        normalized[0::2] = chunk[1::2]
        normalized[1::2] = chunk[0::2]
        chunk = normalized
    elif unit == 4:
        normalized = bytearray(len(chunk))
        normalized[0::4] = chunk[3::4]
        normalized[1::4] = chunk[2::4]
        normalized[2::4] = chunk[1::4]
        normalized[3::4] = chunk[0::4]
        chunk = normalized
    digest.update(chunk)
source.close()
print(digest.hexdigest())
PY
}

if [[ ! -f "$diff_schedule" ]]; then
    print -u2 -- "wm2000 withheld diff: schedule does not exist: $diff_schedule"
    exit 2
fi
for value_name value in \
    MIN_GUEST_INSTRUCTIONS "$diff_minimum" \
    MAX_STEPS "$diff_max_steps"
do
    if [[ ! "$value" =~ '^[1-9][0-9]*$' ]]; then
        print -u2 -- "wm2000 withheld diff: $value_name must be a positive integer"
        exit 2
    fi
done
if [[ ! -d "$diff_output_root" || -L "$diff_output_root" ]]; then
    print -u2 -- "wm2000 withheld diff: output directory does not exist: $diff_output_root"
    exit 2
fi
typeset -r diff_output_canonical=${diff_output_root:A}
if [[ "$diff_output_canonical" == "$diff_root" || "$diff_output_canonical" == "$diff_root"/* ]]; then
    print -u2 -- "wm2000 withheld diff: output artifacts must remain outside the repository"
    exit 2
fi
typeset -a diff_existing_entries
diff_existing_entries=("$diff_output_canonical"/*(DN))
if (( ${#diff_existing_entries} != 0 )); then
    print -u2 -- "wm2000 withheld diff: output directory must be empty"
    exit 2
fi

# The build-local receipt proves only what its producer recorded. Re-derive
# the identities this runner can observe: current inputs and retained binary
# bytes. Capture-group, feature-graph, manifest, and lock identities remain
# receipt assertions and therefore carry no independent authority here.
typeset -r diff_receipt=${FN64_WM_PAIR_RECEIPT:A}
typeset -r diff_receipt_pre_sha=$(hash_file "$diff_receipt")
typeset -r diff_rom_pre_sha=$(hash_file "$ROM")
typeset -r diff_normalized_rom_pre_sha=$(normalized_rom_hash "$ROM")
typeset -r diff_boot_context_pre_sha=$(hash_file "$FN64_BOOT_CONTEXT")
typeset -r diff_schedule_pre_sha=$(hash_file "$diff_schedule")
typeset -r diff_aot_binary_pre_sha=$(hash_file "$FN64_WM_AOT_BINARY")
typeset -r diff_dynamic_binary_pre_sha=$(hash_file "$FN64_WM_DYNAMIC_BINARY")
typeset diff_receipt_schema
diff_receipt_schema=$(python3 - "$diff_receipt" "$diff_rom_pre_sha" "$diff_boot_context_pre_sha" \
    "$diff_aot_binary_pre_sha" "$diff_dynamic_binary_pre_sha" <<'PY'
import json
import sys

receipt_path, rom_sha, boot_sha, aot_sha, dynamic_sha = sys.argv[1:]
try:
    with open(receipt_path, "r", encoding="utf-8") as source:
        receipt = json.load(source)
except (OSError, UnicodeError, json.JSONDecodeError) as error:
    raise SystemExit(f"invalid pair receipt: unreadable JSON: {error}")

def require(condition, message):
    if not condition:
        raise SystemExit(f"invalid pair receipt: {message}")

def is_sha256(value):
    return isinstance(value, str) and len(value) == 64 and all(c in "0123456789abcdef" for c in value)

def exact(value, keys, label):
    require(isinstance(value, dict) and set(value) == set(keys), f"{label} fields")
    return value

require(isinstance(receipt, dict), "root must be an object")
exact(receipt, {
    "schema", "authority", "evidence_scope", "privacy", "identity_relation",
    "inputs", "artifacts", "guard", "commands",
}, "root")
schema = receipt.get("schema")
require(schema in {
    "fn64.wm2000.withheld-pair-build-receipt.v3",
    "fn64.wm2000.withheld-pair-build-receipt.v4",
}, "schema")
is_v3 = schema.endswith(".v3")
require(receipt.get("authority") == "build_local_non_authoritative", "authority")
require(receipt.get("evidence_scope") == "operational_build_local_artifact_identity_only", "evidence scope")
require(receipt.get("identity_relation") == "pre_use_hashes_verified_unchanged_after_both_builds", "identity relation")
require(receipt.get("privacy") == "path-free-private-input-identities-only", "privacy label")
inputs = exact(receipt.get("inputs"), {
    "raw_rom_sha256", "boot_context_sha256", "executable_image_capture_groups",
    "manifest_sha256", "lock_sha256",
}, "inputs")
for key in ("raw_rom_sha256", "boot_context_sha256", "manifest_sha256", "lock_sha256"):
    require(is_sha256(inputs.get(key)), f"input {key}")
groups = inputs.get("executable_image_capture_groups")
require(isinstance(groups, list) and groups, "executable-image capture groups")
for index, group in enumerate(groups):
    require(isinstance(group, dict), f"capture group {index}")
    require(set(group) == {"name", "ordered_capture_sha256"}, f"capture group {index} fields")
    require(isinstance(group["name"], str) and group["name"], f"capture group {index} name")
    captures = group["ordered_capture_sha256"]
    require(isinstance(captures, list) and len(captures) >= 3, f"capture group {index} inventory")
    require(all(is_sha256(value) for value in captures), f"capture group {index} identities")
artifact_keys = {
    "aot_sha256", "dynamic_withheld_sha256", "aot_feature_tree_log_sha256",
    "pure_aot_checker_log_sha256", "dynamic_feature_tree_log_sha256", "target_subdir",
    "cargo_cache_seed",
}
if is_v3:
    artifact_keys.add("dynamic_source_check_log_sha256")
artifacts = exact(receipt.get("artifacts"), artifact_keys, "artifacts")
artifact_digest_keys = {
    "aot_sha256", "dynamic_withheld_sha256", "aot_feature_tree_log_sha256",
    "pure_aot_checker_log_sha256", "dynamic_feature_tree_log_sha256",
}
if is_v3:
    artifact_digest_keys.add("dynamic_source_check_log_sha256")
for key in artifact_digest_keys:
    require(is_sha256(artifacts.get(key)), f"artifact {key}")
require(artifacts.get("target_subdir") == "cargo-target", "artifact target subdirectory")
require(artifacts.get("cargo_cache_seed") in ("none", "caller_provided_untrusted_acceleration"), "artifact Cargo cache seed mode")
guard_keys = {
    "max_rss_mib", "min_free_percent", "max_seconds", "poll_seconds",
    "cargo_build_jobs", "memory_guard_sha256", "pure_aot_checker_sha256",
    "cargo_sha256", "aot_feature_check_jsonl", "dynamic_feature_check_jsonl",
    "aot_jsonl", "dynamic_withheld_jsonl",
    "aot_feature_check_measurement", "dynamic_feature_check_measurement",
    "aot_measurement", "dynamic_withheld_measurement",
}
if is_v3:
    guard_keys.update({"dynamic_source_check_jsonl", "dynamic_source_check_measurement"})
guard = exact(receipt.get("guard"), guard_keys, "guard")
def is_uint(value):
    return isinstance(value, int) and not isinstance(value, bool) and value >= 0
require(is_uint(guard["max_rss_mib"]) and guard["max_rss_mib"] > 0, "guard max RSS")
require(is_uint(guard["min_free_percent"]) and guard["min_free_percent"] <= 100, "guard free percent")
require(is_uint(guard["max_seconds"]) and guard["max_seconds"] > 0, "guard max seconds")
require(guard["poll_seconds"] in ("0.05", "0.1", "0.25", "0.5", "1", "2"), "guard poll interval")
require(guard["cargo_build_jobs"] == 1, "guard Cargo jobs")
for key in ("memory_guard_sha256", "pure_aot_checker_sha256", "cargo_sha256"):
    require(is_sha256(guard[key]), f"guard {key}")
guard_jsonl = {
    "aot_feature_check_jsonl": "aot-feature-check-memory-guard.jsonl",
    "dynamic_feature_check_jsonl": "dynamic-feature-check-memory-guard.jsonl",
    "aot_jsonl": "aot-memory-guard.jsonl",
    "dynamic_withheld_jsonl": "dynamic-withheld-memory-guard.jsonl",
}
if is_v3:
    guard_jsonl["dynamic_source_check_jsonl"] = "dynamic-source-check-memory-guard.jsonl"
for key, expected in guard_jsonl.items():
    require(guard[key] == expected, f"guard {key}")
measurement_keys = {
    "aot_feature_check_measurement", "dynamic_feature_check_measurement",
    "aot_measurement", "dynamic_withheld_measurement",
}
if is_v3:
    measurement_keys.add("dynamic_source_check_measurement")
for key in measurement_keys:
    measurement = exact(guard[key], {"samples", "elapsed_seconds", "peak_tree_rss_mib", "last_reason"}, key)
    require(is_uint(measurement["samples"]) and measurement["samples"] > 0, f"{key} samples")
    require(is_uint(measurement["elapsed_seconds"]), f"{key} elapsed time")
    require(is_uint(measurement["peak_tree_rss_mib"]), f"{key} peak RSS")
    require(measurement["last_reason"] == "complete", f"{key} completion")
command_keys = {
    "pure_aot_feature_gate", "pure_aot_checker_gate", "dynamic_feature_gate",
    "aot", "dynamic_withheld",
}
if is_v3:
    command_keys.add("dynamic_source_check")
commands = exact(receipt.get("commands"), command_keys, "commands")
require(all(isinstance(command, list) and command and all(isinstance(word, str) and word for word in command)
            for command in commands.values()), "command vectors")
require(commands["aot"][-2:] == ["-p", "wm2000-block-boot"], "AOT command suffix")
require(commands["dynamic_withheld"][-4:] == ["-p", "wm2000-block-boot", "--features", "dynamic-withheld"], "dynamic command suffix")
require(commands["dynamic_feature_gate"][-2:] == ["--features", "dynamic-withheld"], "dynamic feature-gate suffix")
if is_v3:
    require(commands["dynamic_source_check"][-4:] == ["--bin", "wm2000-block-boot", "--features", "dynamic-withheld"], "dynamic source-check suffix")
require(inputs["raw_rom_sha256"] == rom_sha, "raw ROM identity")
require(inputs["boot_context_sha256"] == boot_sha, "BootContext identity")
require(artifacts["aot_sha256"] == aot_sha, "AOT executable identity")
require(artifacts["dynamic_withheld_sha256"] == dynamic_sha, "dynamic-withheld executable identity")
require(aot_sha != dynamic_sha, "feature-separated executable identities")
print(schema)
PY
) || exit 1
typeset -r diff_receipt_schema

# Prevent caller-shell diagnostics from creating unbounded histories or
# changing the two lane inputs. The controller schedule remains explicit.
unset FN64_BLOCK_EXECUTOR_TRACE
unset FN64_BLOCK_DEVICE_TRACE
unset FN64_BLOCK_PC_TRACE
unset FN64_BLOCK_HOST_TRACE
unset FN64_PROFILE_STOP_AT_GENERATION
unset FN64_PROFILE_STOP_AT_AOT_PC
unset FN64_BLOCK_EXPECT_GUEST_INSTRUCTIONS
unset FN64_DYNAMIC_WITHHOLD_BOOT_SHARD
unset FN64_DYNAMIC_WITHHOLD_CANONICAL_ENTRY
unset FN64_DYNAMIC_TELEMETRY
unset FN64_WM_PUBLICATION_DIAGNOSTIC
export FN64_GUARD_MAX_RSS_MIB=$diff_guard_max_rss_mib

diff_common_environment=(
    "ROM=$ROM"
    "FN64_BOOT_CONTEXT=$FN64_BOOT_CONTEXT"
    "FN64_CONTROLLER_SCHEDULE=$diff_schedule"
    "FN64_BLOCK_MIN_GUEST_INSTRUCTIONS=$diff_minimum"
    "FN64_BLOCK_MAX_STEPS=$diff_max_steps"
    FN64_BLOCK_CONTINUE_AFTER_OVERLAY=1
    FN64_BLOCK_PROGRESS_ONLY=1
    FN64_WM_PUBLICATION_DIAGNOSTIC=1
)

print -u2 -- "wm2000 withheld diff: AOT baseline"
if ! "$diff_root/scripts/memory-guard.zsh" /usr/bin/env \
    "${diff_common_environment[@]}" "$FN64_WM_AOT_BINARY" \
    >"$diff_aot_log" 2>&1
then
    print -u2 -- "wm2000 withheld diff: AOT baseline failed; retained $diff_aot_log"
    exit 1
fi

typeset -a diff_checkpoint_lines
diff_checkpoint_lines=("${(@f)$(rg '^\[wm2000-block-checkpoint\] ' "$diff_aot_log")}")
if (( ${#diff_checkpoint_lines} != 1 )); then
    print -u2 -- "wm2000 withheld diff: expected one AOT checkpoint line; retained $diff_aot_log"
    exit 1
fi
typeset -r diff_checkpoint=${diff_checkpoint_lines[1]}
if [[ ! "$diff_checkpoint" =~ '^\[wm2000-block-checkpoint\] minimum_guest_instructions=([0-9]+) expected_guest_instructions=None achieved_guest_instructions=([0-9]+) scheduler_steps=([0-9]+) sim_time=([0-9]+) logical_rdram_bytes=([0-9]+) logical_rdram_sha256=([0-9a-f]{64})$' ]]; then
    print -u2 -- "wm2000 withheld diff: malformed AOT checkpoint evidence; retained $diff_aot_log"
    exit 1
fi
typeset -r diff_aot_minimum=$match[1]
typeset -r diff_aot_achieved=$match[2]
typeset -r diff_aot_steps=$match[3]
typeset -r diff_aot_sim_time=$match[4]
typeset -r diff_aot_rdram_bytes=$match[5]
typeset -r diff_aot_rdram_sha=$match[6]
if (( diff_aot_minimum != diff_minimum || diff_aot_achieved < diff_minimum \
    || diff_aot_rdram_bytes != 8388608 )); then
    print -u2 -- "wm2000 withheld diff: AOT checkpoint does not bind the requested minimum/full RDRAM; retained $diff_aot_log"
    exit 1
fi

typeset -a diff_aot_program_identity_lines
diff_aot_program_identity_lines=("${(@f)$(rg '^\[wm2000-program-identity\] ' "$diff_aot_log")}")
if (( ${#diff_aot_program_identity_lines} != 1 )); then
    print -u2 -- "wm2000 withheld diff: expected one AOT program-identity line; retained $diff_aot_log"
    exit 1
fi
if [[ ! "${diff_aot_program_identity_lines[1]}" =~ '^\[wm2000-program-identity\] schema=fn64\.wm2000\.program-identity\.v1 sha256=([0-9a-f]{64}) source=(caller_supplied|canonical_block_program_sha256) resolver_sha256=([0-9a-f]{64}) entry_bank=([0-9a-f]{16}) entry_pc=([0-9a-f]{8})$' ]]; then
    print -u2 -- "wm2000 withheld diff: malformed AOT program identity; retained $diff_aot_log"
    exit 1
fi
typeset -r diff_aot_program_sha=$match[1]
typeset -r diff_aot_program_source=$match[2]
typeset -r diff_aot_resolver_sha=$match[3]
typeset -r diff_aot_entry_bank=$match[4]
typeset -r diff_aot_entry_pc=$match[5]

typeset -a diff_operational_lines
diff_operational_lines=("${(@f)$(rg '^\[wm2000-operational-boundary\] ' "$diff_aot_log")}")
if (( ${#diff_operational_lines} != 1 )); then
    print -u2 -- "wm2000 withheld diff: expected one AOT operational-boundary line; retained $diff_aot_log"
    exit 1
fi
typeset -r diff_operational=${diff_operational_lines[1]}
if [[ ! "$diff_operational" =~ '^\[wm2000-operational-boundary\] schema=fn64\.wm2000\.operational-boundary\.v1 component_schema=fn64\.operational-state-component-digests\.v1 publication_schema=fn64\.operational-thread-publication-digests\.v2 capture_relation=latest_per_thread_publication_paired_with_post_scheduler_owner_snapshots device_sha256=([0-9a-f]{64}) executor_sha256=([0-9a-f]{64}) abi_host_sha256=([0-9a-f]{64}) cpu_sha256=([0-9a-f]{64}) continuation_sha256=([0-9a-f]{64}) executor_threads=([0-9]+) publications=([0-9]+) exact=([0-9]+) opaque=([0-9]+) opaque_host=([0-9]+) parked_fault=([0-9]+) returned=([0-9]+) missing=([0-9]+) unexpected=([0-9]+) cpu_comparable=(true|false) mutation_sealed=(true|false) pending_writes=([0-9]+) open_host_transactions=([0-9]+) mutation_quiescent=(true|false)$' ]]; then
    print -u2 -- "wm2000 withheld diff: malformed AOT operational-boundary evidence; retained $diff_aot_log"
    exit 1
fi
typeset -r diff_aot_device_sha=$match[1]
typeset -r diff_aot_executor_sha=$match[2]
typeset -r diff_aot_abi_host_sha=$match[3]
typeset -r diff_aot_cpu_sha=$match[4]
typeset -r diff_aot_continuation_sha=$match[5]
typeset -r diff_aot_executor_threads=$match[6]
typeset -r diff_aot_publications=$match[7]
typeset -r diff_aot_exact=$match[8]
typeset -r diff_aot_opaque=$match[9]
typeset -r diff_aot_opaque_host=$match[10]
typeset -r diff_aot_parked_fault=$match[11]
typeset -r diff_aot_returned=$match[12]
typeset -r diff_aot_missing=$match[13]
typeset -r diff_aot_unexpected=$match[14]
typeset -r diff_aot_cpu_comparable=$match[15]
typeset -r diff_aot_mutation_sealed=$match[16]
typeset -r diff_aot_pending_writes=$match[17]
typeset -r diff_aot_open_host_transactions=$match[18]
typeset -r diff_aot_mutation_quiescent=$match[19]
typeset diff_aot_expected_cpu_comparable=false
if (( diff_aot_missing == 0 && diff_aot_unexpected == 0 && diff_aot_opaque == 0 )); then
    diff_aot_expected_cpu_comparable=true
fi
if (( diff_aot_opaque != diff_aot_opaque_host + diff_aot_parked_fault \
    || diff_aot_publications != diff_aot_exact + diff_aot_opaque + diff_aot_returned )) \
    || (( diff_aot_executor_threads + diff_aot_unexpected != diff_aot_publications + diff_aot_missing )) \
    || [[ "$diff_aot_mutation_sealed" != true || "$diff_aot_pending_writes" != 0 \
        || "$diff_aot_open_host_transactions" != 0 || "$diff_aot_mutation_quiescent" != true \
        || "$diff_aot_cpu_comparable" != "$diff_aot_expected_cpu_comparable" ]]
then
    print -u2 -- "wm2000 withheld diff: inconsistent or non-quiescent AOT operational boundary; retained $diff_aot_log"
    exit 1
fi

print -u2 -- "wm2000 withheld diff: dynamic exact canonical entry at guest-work count $diff_aot_achieved"
if ! "$diff_root/scripts/memory-guard.zsh" /usr/bin/env \
    "${diff_common_environment[@]}" \
    "FN64_BLOCK_MIN_GUEST_INSTRUCTIONS=$diff_aot_achieved" \
    "FN64_BLOCK_EXPECT_GUEST_INSTRUCTIONS=$diff_aot_achieved" \
    FN64_DYNAMIC_WITHHOLD_CANONICAL_ENTRY=1 \
    "FN64_DYNAMIC_TELEMETRY=$diff_dynamic_telemetry" \
    "$FN64_WM_DYNAMIC_BINARY" >"$diff_dynamic_log" 2>&1
then
    print -u2 -- "wm2000 withheld diff: dynamic lane failed; retained $diff_dynamic_log"
    exit 1
fi

typeset -a diff_dynamic_program_identity_lines
diff_dynamic_program_identity_lines=("${(@f)$(rg '^\[wm2000-program-identity\] ' "$diff_dynamic_log")}")
if (( ${#diff_dynamic_program_identity_lines} != 1 )); then
    print -u2 -- "wm2000 withheld diff: expected one dynamic program-identity line; retained $diff_dynamic_log"
    exit 1
fi
if [[ ! "${diff_dynamic_program_identity_lines[1]}" =~ '^\[wm2000-program-identity\] schema=fn64\.wm2000\.program-identity\.v1 sha256=([0-9a-f]{64}) source=(caller_supplied|canonical_block_program_sha256) resolver_sha256=([0-9a-f]{64}) entry_bank=([0-9a-f]{16}) entry_pc=([0-9a-f]{8})$' ]]; then
    print -u2 -- "wm2000 withheld diff: malformed dynamic program identity; retained $diff_dynamic_log"
    exit 1
fi
typeset -r diff_dynamic_program_sha=$match[1]
typeset -r diff_dynamic_program_source=$match[2]
typeset -r diff_dynamic_resolver_sha=$match[3]
typeset -r diff_dynamic_entry_bank=$match[4]
typeset -r diff_dynamic_entry_pc=$match[5]
if [[ "$diff_dynamic_program_sha" != "$diff_aot_program_sha" \
    || "$diff_dynamic_program_source" != "$diff_aot_program_source" \
    || "$diff_dynamic_resolver_sha" != "$diff_aot_resolver_sha" \
    || "$diff_dynamic_entry_bank" != "$diff_aot_entry_bank" \
    || "$diff_dynamic_entry_pc" != "$diff_aot_entry_pc" ]]
then
    print -u2 -- "wm2000 withheld diff: AOT/dynamic program identity drift; retained $diff_aot_log and $diff_dynamic_log"
    exit 1
fi

verify_unchanged() {
    local label=$1
    local input_path=$2
    local expected=$3
    if [[ "$(hash_file "$input_path")" != "$expected" ]]; then
        print -u2 -- "wm2000 withheld diff: $label changed during comparison; refusing evidence"
        exit 1
    fi
}
verify_unchanged pair-receipt "$diff_receipt" "$diff_receipt_pre_sha"
verify_unchanged ROM "$ROM" "$diff_rom_pre_sha"
if [[ "$(normalized_rom_hash "$ROM")" != "$diff_normalized_rom_pre_sha" ]]; then
    print -u2 -- "wm2000 withheld diff: normalized ROM identity changed during comparison; refusing evidence"
    exit 1
fi
verify_unchanged FN64_BOOT_CONTEXT "$FN64_BOOT_CONTEXT" "$diff_boot_context_pre_sha"
verify_unchanged controller-schedule "$diff_schedule" "$diff_schedule_pre_sha"
verify_unchanged AOT-executable "$FN64_WM_AOT_BINARY" "$diff_aot_binary_pre_sha"
verify_unchanged dynamic-withheld-executable "$FN64_WM_DYNAMIC_BINARY" "$diff_dynamic_binary_pre_sha"
python3 -c '
import json
import re
import sys

(
    telemetry_path,
    comparison_path,
    expected_instructions,
    expected_rdram,
    expected_rom,
    expected_program_sha,
    expected_program_source,
    expected_resolver_sha,
    expected_entry_bank,
    expected_entry_pc,
    aot_steps,
    aot_sim_time,
    aot_device_sha,
    aot_executor_sha,
    aot_abi_host_sha,
    aot_cpu_sha,
    aot_continuation_sha,
    aot_executor_threads,
    aot_publications,
    aot_exact,
    aot_opaque,
    aot_opaque_host,
    aot_parked_fault,
    aot_returned,
    aot_missing,
    aot_unexpected,
    aot_cpu_comparable,
    pair_receipt_sha,
    pair_receipt_schema,
    raw_rom_sha,
    boot_context_sha,
    schedule_sha,
    aot_binary_sha,
    dynamic_binary_sha,
    aot_log_path,
    dynamic_log_path,
) = sys.argv[1:]
with open(telemetry_path, "r", encoding="utf-8") as source:
    telemetry = json.load(source)

def require(condition, message):
    if not condition:
        raise SystemExit(f"invalid dynamic telemetry: {message}")

def is_sha256(value):
    return isinstance(value, str) and len(value) == 64 and all(c in "0123456789abcdef" for c in value)

def is_uint(value):
    return isinstance(value, int) and not isinstance(value, bool) and value >= 0

def parse_pc(value, label):
    require(isinstance(value, str) and re.fullmatch(r"[0-9a-f]{8}", value) is not None, label)
    return int(value, 16)

def exact_fields(value, fields, label):
    require(isinstance(value, dict) and set(value) == set(fields), f"{label} fields")
    return value

def is_hex(value, digits):
    return isinstance(value, str) and re.fullmatch(rf"0x[0-9a-f]{{{digits}}}", value) is not None

def execution_key(value, label):
    value = exact_fields(value, {"bank", "pc"}, label)
    require(is_hex(value["bank"], 16), f"{label} bank")
    require(is_hex(value["pc"], 8), f"{label} PC")

def fault(value, label):
    value = exact_fields(value, {"at", "kind"}, label)
    execution_key(value["at"], f"{label} at")
    require(isinstance(value["kind"], str) and value["kind"], f"{label} kind")

def pending_exit(value, label):
    require(isinstance(value, dict), f"{label} object")
    variant = value.get("variant")
    fields = {
        "transfer": {"variant", "entry"},
        "resolve_transfer": {"variant", "source_bank", "target_pc"},
        "resolve_call": {"variant", "source_bank", "target_pc", "resume"},
        "host_call": {"variant", "target_pc", "resume"},
        "executable_write": {"variant", "source_bank", "resume"},
        "executable_write_resolve_call": {
            "variant", "source_bank", "target_pc", "resume",
        },
        "executable_write_fault": {"variant", "fault"},
        "image_changed": {
            "variant", "at", "expected_bank", "va_start", "byte_len",
            "expected_sha256", "actual_sha256",
        },
        "checkpoint": {"variant", "entry"},
        "yield": {"variant", "entry"},
        "thread_return": {"variant"},
        "fault": {"variant", "fault"},
    }
    require(variant in fields, f"{label} variant")
    exact_fields(value, fields[variant], label)
    if "entry" in value:
        execution_key(value["entry"], f"{label} entry")
    if "resume" in value:
        execution_key(value["resume"], f"{label} resume")
    if "at" in value:
        execution_key(value["at"], f"{label} at")
    if "fault" in value:
        fault(value["fault"], f"{label} fault")
    if "source_bank" in value:
        require(is_hex(value["source_bank"], 16), f"{label} source bank")
    for key in ("target_pc", "va_start"):
        if key in value:
            require(is_hex(value[key], 8), f"{label} {key}")
    if "expected_bank" in value:
        require(is_hex(value["expected_bank"], 16), f"{label} expected bank")
    if "byte_len" in value:
        require(is_uint(value["byte_len"]), f"{label} byte length")
    for key in ("expected_sha256", "actual_sha256"):
        if key in value:
            require(is_sha256(value[key]), f"{label} {key}")

def prepared_continuation(value, label):
    if value is None:
        return
    value = exact_fields(value, {"variant", "entry"}, label)
    require(value["variant"] in ("image_changed", "inactive_generation"), f"{label} variant")
    execution_key(value["entry"], f"{label} entry")

def exact_continuation_coherent(exit_value, prepared, label):
    if prepared is None:
        requires_prepared = exit_value["variant"] == "image_changed" or (
            exit_value["variant"] == "fault"
            and exit_value["fault"]["kind"] == "NoActiveGeneration"
        )
        require(not requires_prepared, f"{label} missing prepared continuation")
    elif prepared["variant"] == "image_changed":
        require(exit_value["variant"] == "image_changed", f"{label} image-changed relation")
        require(prepared["entry"]["pc"] == exit_value["at"]["pc"], f"{label} image-changed PC")
    else:
        require(
            exit_value["variant"] == "fault"
            and exit_value["fault"]["kind"] == "NoActiveGeneration",
            f"{label} inactive-generation relation",
        )
        require(
            prepared["entry"]["pc"] == exit_value["fault"]["at"]["pc"],
            f"{label} inactive-generation PC",
        )

def diagnostic_cpu(value, label):
    value = exact_fields(value, {
        "cop0_count", "cop0_compare", "cop0_count_write", "cop0_compare_write",
        "count_independent_schema", "count_independent_sha256",
    }, label)
    require(is_hex(value["cop0_count"], 8), f"{label} Count")
    require(is_hex(value["cop0_compare"], 8), f"{label} Compare")
    for key in ("cop0_count_write", "cop0_compare_write"):
        require(value[key] is None or is_hex(value[key], 8), f"{label} {key}")
    require(
        value["count_independent_schema"]
        == "fn64.wm2000.operational-cpu-count-independent.v1",
        f"{label} count-independent schema",
    )
    require(is_sha256(value["count_independent_sha256"]), f"{label} count-independent identity")

def publication_diagnostics(path, lane, expected_profile):
    prefix = "[wm2000-publication-diagnostic] "
    records = []
    try:
        with open(path, "r", encoding="utf-8") as source:
            for line in source:
                if line.startswith(prefix):
                    try:
                        def reject_duplicate_fields(pairs):
                            result = {}
                            for key, value in pairs:
                                if key in result:
                                    raise ValueError(f"duplicate field {key!r}")
                                result[key] = value
                            return result
                        records.append(json.loads(
                            line[len(prefix):],
                            object_pairs_hook=reject_duplicate_fields,
                        ))
                    except (json.JSONDecodeError, ValueError) as error:
                        require(False, f"{lane} publication diagnostic JSON: {error}")
    except (OSError, UnicodeError) as error:
        require(False, f"{lane} publication diagnostic log: {error}")
    prior_thread = None
    variant_counts = {
        "exact": 0,
        "opaque_host_in_flight": 0,
        "parked_fault_opaque": 0,
        "returned": 0,
    }
    exact_records = []
    for index, record in enumerate(records):
        label = f"{lane} publication diagnostic {index}"
        require(isinstance(record, dict), f"{label} object")
        variant = record.get("publication_variant")
        field_sets = {
            "exact": {
                "schema", "thread", "publication_variant", "last_charge",
                "cumulative_charge", "pending_exit", "prepared_continuation", "cpu",
            },
            "opaque_host_in_flight": {
                "schema", "thread", "publication_variant", "target_pc", "resume",
            },
            "parked_fault_opaque": {
                "schema", "thread", "publication_variant", "cumulative_charge", "fault", "cpu",
            },
            "returned": {"schema", "thread", "publication_variant", "cpu"},
        }
        require(variant in field_sets, f"{label} variant")
        exact_fields(record, field_sets[variant], label)
        require(record["schema"] == "fn64.wm2000.publication-diagnostic.v1", f"{label} schema")
        require(is_uint(record["thread"]), f"{label} thread")
        require(prior_thread is None or record["thread"] > prior_thread, f"{lane} publication diagnostic thread ordering")
        prior_thread = record["thread"]
        variant_counts[variant] += 1
        if variant == "exact":
            require(is_uint(record["last_charge"]) and record["last_charge"] > 0, f"{label} last charge")
            require(is_uint(record["cumulative_charge"]), f"{label} cumulative charge")
            require(record["last_charge"] <= record["cumulative_charge"], f"{label} charge relation")
            pending_exit(record["pending_exit"], f"{label} pending exit")
            prepared_continuation(record["prepared_continuation"], f"{label} prepared continuation")
            exact_continuation_coherent(
                record["pending_exit"], record["prepared_continuation"], label,
            )
            diagnostic_cpu(record["cpu"], f"{label} CPU")
            exact_records.append(record)
        elif variant == "opaque_host_in_flight":
            require(is_hex(record["target_pc"], 8), f"{label} target PC")
            execution_key(record["resume"], f"{label} resume")
        elif variant == "parked_fault_opaque":
            require(is_uint(record["cumulative_charge"]), f"{label} cumulative charge")
            fault(record["fault"], f"{label} fault")
            diagnostic_cpu(record["cpu"], f"{label} CPU")
        else:
            diagnostic_cpu(record["cpu"], f"{label} CPU")
    require(len(records) == expected_profile[1], f"{lane} publication diagnostic count")
    require(variant_counts["exact"] == expected_profile[2], f"{lane} exact diagnostic count")
    require(
        variant_counts["opaque_host_in_flight"] == expected_profile[4],
        f"{lane} opaque-host diagnostic count",
    )
    require(
        variant_counts["parked_fault_opaque"] == expected_profile[5],
        f"{lane} parked-fault diagnostic count",
    )
    require(variant_counts["returned"] == expected_profile[6], f"{lane} returned diagnostic count")
    return exact_records

require(telemetry.get("schema") == "fn64.wm2000.dynamic-withheld-telemetry.v2", "schema")
require(telemetry.get("authority") == "operational_only_dynamic_installed", "authority")
require(telemetry.get("claim") == "dynamically_executed_exact_withheld_static_key", "claim")
require(telemetry.get("selection_basis") == "validated_canonical_catalog_entry", "selection basis")
require(telemetry.get("resolver_install_sha256") == expected_resolver_sha, "resolver identity binding")
require(is_sha256(telemetry.get("dynamic_source_sha256")), "dynamic source identity")
require(telemetry.get("rom_sha256") == expected_rom, "ROM identity")
require(is_sha256(telemetry.get("bootstrap_receipt_sha256")), "bootstrap identity")
require(is_sha256(telemetry.get("mutation_journal_root_sha256")), "mutation root")
program_identity = telemetry.get("program_identity", {})
require(is_sha256(program_identity.get("sha256")), "program identity")
require(program_identity.get("sha256") == expected_program_sha, "program identity binding")
require(program_identity.get("source") == expected_program_source, "program identity source binding")
require(expected_program_source == "canonical_block_program_sha256", "canonical program identity source")

withheld = telemetry.get("withheld", {})
withheld_bank = withheld.get("bank")
withheld_pc = withheld.get("pc")
require(isinstance(withheld_bank, str) and re.fullmatch(r"[0-9a-f]{16}", withheld_bank) is not None, "withheld bank")
parse_pc(withheld_pc, "withheld PC")
require(withheld_bank == expected_entry_bank and withheld_pc == expected_entry_pc, "withheld key equals canonical program entry")

horizon = telemetry.get("guest_instruction_horizon", {})
actual_instructions = horizon.get("achieved")
require(is_uint(horizon.get("minimum")) and horizon.get("minimum") == int(expected_instructions), "dynamic minimum")
require(is_uint(horizon.get("expected_exact")) and horizon.get("expected_exact") == int(expected_instructions), "dynamic exact target")
require(is_uint(actual_instructions) and actual_instructions == int(expected_instructions), "dynamic achieved count")
require(horizon.get("expected_match") is True, "dynamic exact-match flag")

rdram = telemetry.get("full_logical_rdram", {})
actual_rdram = rdram.get("sha256")
require(is_uint(rdram.get("byte_len")) and rdram.get("byte_len") == 8388608, "full logical RDRAM length")
require(is_sha256(actual_rdram), "full logical RDRAM identity")

for key in (
    "dropped_identity_activations",
    "dropped_identity_charged_instructions",
    "dropped_identity_unsupported_exits",
    "dropped_attempted_entry_activations",
    "dropped_attempted_entry_charged_instructions",
    "dropped_attempted_entry_unsupported_exits",
):
    require(is_uint(telemetry.get(key)) and telemetry.get(key) == 0, key)
mutation = telemetry.get("mutation_quiescence", {})
require(mutation.get("mutation_sealed") is True, "mutation seal")
require(is_uint(mutation.get("pending_attributed_writes")) and mutation.get("pending_attributed_writes") == 0, "pending mutation writes")
require(is_uint(mutation.get("open_host_transactions")) and mutation.get("open_host_transactions") == 0, "open host transactions")
require(mutation.get("mutation_journal_quiescent") is True, "mutation quiescence")

aggregates = telemetry.get("aggregates")
require(isinstance(aggregates, list) and aggregates, "dynamic aggregate inventory")
require(all(isinstance(item, dict) for item in aggregates), "dynamic aggregate object")
require(all(is_uint(item.get("unsupported_exits")) and item.get("unsupported_exits") == 0 for item in aggregates), "unsupported dynamic exits")
exact_withheld_dynamic_work = False
for item in aggregates:
    require(is_uint(item.get("charged_instructions")), "dynamic aggregate charged instructions")
    attempted_entries = item.get("attempted_entries")
    require(isinstance(attempted_entries, list), "dynamic aggregate attempted entries")
    for entry in attempted_entries:
        require(isinstance(entry, dict), "dynamic attempted-entry object")
        bank = entry.get("bank")
        pc = entry.get("pc")
        activations = entry.get("activations")
        charged_instructions = entry.get("charged_instructions")
        unsupported_exits = entry.get("unsupported_exits")
        require(isinstance(bank, str) and re.fullmatch(r"[0-9a-f]{16}", bank) is not None, "dynamic attempted-entry bank")
        parse_pc(pc, "dynamic attempted-entry PC")
        require(is_uint(activations), "dynamic attempted-entry activations")
        require(is_uint(charged_instructions), "dynamic attempted-entry charged instructions")
        require(is_uint(unsupported_exits) and unsupported_exits == 0, "dynamic attempted-entry unsupported exits")
        if (
            bank == expected_entry_bank
            and pc == expected_entry_pc
            and activations > 0
            and charged_instructions > 0
        ):
            exact_withheld_dynamic_work = True
require(exact_withheld_dynamic_work, "charged dynamic execution of exact withheld canonical entry")
termination = telemetry.get("termination", {})
require(termination.get("mode") == "bounded_progress_only", "termination mode")
require(termination.get("stop_cause") == "minimum_guest_instruction_checkpoint_reached", "termination cause")
scheduler = telemetry.get("scheduler_boundary", {})
require(is_uint(scheduler.get("steps")), "dynamic scheduler steps")
require(is_uint(scheduler.get("sim_time")), "dynamic simulator time")

boundary = telemetry.get("operational_boundary", {})
require(boundary.get("schema") == "fn64.wm2000.operational-boundary.v1", "operational boundary schema")
require(boundary.get("component_schema") == "fn64.operational-state-component-digests.v1", "component schema")
require(boundary.get("publication_schema") == "fn64.operational-thread-publication-digests.v2", "publication schema")
require(boundary.get("capture_relation") == "latest_per_thread_publication_paired_with_post_scheduler_owner_snapshots", "capture relation")
components = boundary.get("components", {})
for key in ("device_sha256", "executor_sha256", "abi_host_sha256"):
    require(is_sha256(components.get(key)), f"operational {key}")
publications = boundary.get("thread_publications", {})
for key in (
    "executor_threads",
    "publication_count",
    "exact_count",
    "opaque_count",
    "opaque_host_count",
    "parked_fault_count",
    "returned_count",
    "missing_count",
    "unexpected_count",
):
    require(is_uint(publications.get(key)), f"publication {key}")
require(is_sha256(publications.get("cpu_sha256")), "CPU publication identity")
require(is_sha256(publications.get("continuation_sha256")), "continuation publication identity")
require(isinstance(publications.get("cpu_comparable"), bool), "CPU comparability")
require(
    publications["opaque_count"]
    == publications["opaque_host_count"] + publications["parked_fault_count"],
    "opaque publication variant counts",
)
require(
    publications["publication_count"]
    == publications["exact_count"] + publications["opaque_count"] + publications["returned_count"],
    "publication variant counts",
)
require(
    publications["executor_threads"] + publications["unexpected_count"]
    == publications["publication_count"] + publications["missing_count"],
    "executor/publication set counts",
)
require(
    publications["cpu_comparable"]
    == (
        publications["missing_count"] == 0
        and publications["unexpected_count"] == 0
        and publications["opaque_count"] == 0
    ),
    "CPU comparability derivation",
)
require(boundary.get("mutation_quiescence") == mutation, "operational mutation binding")

aot_publication_profile = (
    int(aot_executor_threads),
    int(aot_publications),
    int(aot_exact),
    int(aot_opaque),
    int(aot_opaque_host),
    int(aot_parked_fault),
    int(aot_returned),
    int(aot_missing),
    int(aot_unexpected),
)
dynamic_publication_profile = (
    publications["executor_threads"],
    publications["publication_count"],
    publications["exact_count"],
    publications["opaque_count"],
    publications["opaque_host_count"],
    publications["parked_fault_count"],
    publications["returned_count"],
    publications["missing_count"],
    publications["unexpected_count"],
)
aot_exact_diagnostics = publication_diagnostics(
    aot_log_path, "AOT", aot_publication_profile,
)
dynamic_exact_diagnostics = publication_diagnostics(
    dynamic_log_path, "dynamic", dynamic_publication_profile,
)

aot_exact_by_thread = {record["thread"]: record for record in aot_exact_diagnostics}
dynamic_exact_by_thread = {record["thread"]: record for record in dynamic_exact_diagnostics}
exact_thread_field_matches = []
diagnostic_fields = (
    "last_charge",
    "cumulative_charge",
    "pending_exit",
    "prepared_continuation",
    "cpu",
)
for thread in sorted(set(aot_exact_by_thread) | set(dynamic_exact_by_thread)):
    aot_record = aot_exact_by_thread.get(thread)
    dynamic_record = dynamic_exact_by_thread.get(thread)
    if aot_record is None:
        classification = "dynamic_only"
    elif dynamic_record is None:
        classification = "aot_only"
    elif all(aot_record[field] == dynamic_record[field] for field in diagnostic_fields):
        classification = "match"
    else:
        classification = "field_mismatch"
    exact_thread_field_matches.append({
        "thread": thread,
        "classification": classification,
        "field_matches": {
            field: (
                aot_record[field] == dynamic_record[field]
                if aot_record is not None and dynamic_record is not None
                else None
            )
            for field in diagnostic_fields
        },
    })
aot_cpu_comparable = aot_cpu_comparable == "true"
cpu_comparable = (
    aot_cpu_comparable
    and publications["cpu_comparable"]
    and aot_publication_profile == dynamic_publication_profile
)
cpu_match = publications["cpu_sha256"] == aot_cpu_sha if cpu_comparable else None
continuation_match = (
    publications["continuation_sha256"] == aot_continuation_sha
    if cpu_comparable
    else None
)
published_cpu_gate_pass = (
    cpu_match is True and continuation_match is True
    if cpu_comparable
    else None
)

comparison = {
    "schema": "fn64.wm2000.withheld-operational-comparison.v3",
    "authority": "operational_rdram_and_owner_components_with_published_cpu_diagnostics",
    "expected_guest_instructions": int(expected_instructions),
    "actual_guest_instructions": actual_instructions,
    "expected_logical_rdram_sha256": expected_rdram,
    "actual_logical_rdram_sha256": actual_rdram,
    "guest_instruction_match": actual_instructions == int(expected_instructions),
    "logical_rdram_match": actual_rdram == expected_rdram,
    "aot_scheduler_steps": int(aot_steps),
    "dynamic_scheduler_steps": scheduler["steps"],
    "scheduler_steps_match": scheduler["steps"] == int(aot_steps),
    "aot_sim_time": int(aot_sim_time),
    "dynamic_sim_time": scheduler["sim_time"],
    "sim_time_match": scheduler["sim_time"] == int(aot_sim_time),
    "expected_device_sha256": aot_device_sha,
    "actual_device_sha256": components["device_sha256"],
    "device_match": components["device_sha256"] == aot_device_sha,
    "expected_executor_sha256": aot_executor_sha,
    "actual_executor_sha256": components["executor_sha256"],
    "executor_match": components["executor_sha256"] == aot_executor_sha,
    "expected_abi_host_sha256": aot_abi_host_sha,
    "actual_abi_host_sha256": components["abi_host_sha256"],
    "abi_host_match": components["abi_host_sha256"] == aot_abi_host_sha,
    "aot_publication_profile": aot_publication_profile,
    "dynamic_publication_profile": dynamic_publication_profile,
    "publication_diagnostic": {
        "schema": "fn64.wm2000.publication-diagnostic-comparison.v1",
        "authority": "diagnostic_only_canonical_publication_digests_remain_the_gate",
        "last_charge_relation": "diagnostic_only_slice_partitioning_may_differ",
        "aot_exact_thread_projections": aot_exact_diagnostics,
        "dynamic_exact_thread_projections": dynamic_exact_diagnostics,
        "exact_thread_field_matches": exact_thread_field_matches,
    },
    "cpu_comparable": cpu_comparable,
    "expected_cpu_sha256": aot_cpu_sha,
    "actual_cpu_sha256": publications["cpu_sha256"],
    "cpu_match": cpu_match,
    "expected_continuation_sha256": aot_continuation_sha,
    "actual_continuation_sha256": publications["continuation_sha256"],
    "continuation_match": continuation_match,
    "published_cpu_gate_pass": published_cpu_gate_pass,
    "pair_provenance": {
        "receipt_schema": pair_receipt_schema,
        "receipt_authority": "build_local_non_authoritative",
        "receipt_sha256": pair_receipt_sha,
        "identity_relation": "receipt_artifacts_and_inputs_prevalidated_then_runtime_inputs_pre_post_verified",
        "raw_rom_sha256": raw_rom_sha,
        "normalized_rom_sha256": expected_rom,
        "boot_context_sha256": boot_context_sha,
        "controller_schedule_sha256": schedule_sha,
        "aot_executable_sha256": aot_binary_sha,
        "dynamic_withheld_executable_sha256": dynamic_binary_sha,
        "limitation": "capture_group_manifest_lock_and_feature_graph_identities_are_non_authoritative_build_receipt_assertions_not_independently_rederived",
    },
}
comparison["partial_owner_match"] = all((
    comparison["guest_instruction_match"],
    comparison["logical_rdram_match"],
    comparison["device_match"],
    comparison["executor_match"],
    comparison["abi_host_match"],
))
comparison["operational_match"] = (
    comparison["partial_owner_match"] and comparison["published_cpu_gate_pass"]
    if cpu_comparable
    else None
)
# Retain the existing field for consumers, but never turn incomplete CPU
# evidence into a successful comparison.
comparison["match"] = comparison["operational_match"]
with open(comparison_path, "x", encoding="utf-8") as output:
    json.dump(comparison, output, sort_keys=True, indent=2)
    output.write("\n")
if comparison["operational_match"] is None:
    raise SystemExit("operational comparison unproven: CPU publications are not comparable")
if not comparison["operational_match"]:
    raise SystemExit("operational comparison mismatch")
' "$diff_dynamic_telemetry" "$diff_comparison" "$diff_aot_achieved" "$diff_aot_rdram_sha" "$diff_normalized_rom_pre_sha" "$diff_aot_program_sha" "$diff_aot_program_source" "$diff_aot_resolver_sha" "$diff_aot_entry_bank" "$diff_aot_entry_pc" "$diff_aot_steps" "$diff_aot_sim_time" "$diff_aot_device_sha" "$diff_aot_executor_sha" "$diff_aot_abi_host_sha" "$diff_aot_cpu_sha" "$diff_aot_continuation_sha" "$diff_aot_executor_threads" "$diff_aot_publications" "$diff_aot_exact" "$diff_aot_opaque" "$diff_aot_opaque_host" "$diff_aot_parked_fault" "$diff_aot_returned" "$diff_aot_missing" "$diff_aot_unexpected" "$diff_aot_cpu_comparable" "$diff_receipt_pre_sha" "$diff_receipt_schema" "$diff_rom_pre_sha" "$diff_boot_context_pre_sha" "$diff_schedule_pre_sha" "$diff_aot_binary_pre_sha" "$diff_dynamic_binary_pre_sha" "$diff_aot_log" "$diff_dynamic_log" || {
    print -u2 -- "wm2000 withheld diff: operational comparison failed; retained artifacts in $diff_output_canonical"
    exit 1
}

print -- "wm2000 withheld diff: MATCH guest_instructions=$diff_aot_achieved logical_rdram_sha256=$diff_aot_rdram_sha owner_components=device,executor,abi-host"
print -- "wm2000 withheld diff: artifacts=$diff_output_canonical"
