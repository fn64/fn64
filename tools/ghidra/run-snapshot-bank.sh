#!/bin/sh
set -eu
umask 077

repo=$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd -P)
runner_source="$repo/tools/ghidra/run-snapshot-bank.sh"
guard="$repo/scripts/memory-guard.zsh"
seed_source="$repo/tools/ghidra/Fn64SeedFunctions.java"
export_source="$repo/tools/ghidra/Fn64ExportCandidates.java"
distribution_manifest_source="$repo/tools/ghidra/manifest-ghidra-distribution.py"

fail() {
    echo "ghidra snapshot-bank: $1" >&2
    exit 2
}

case "$0" in
    /*) invoked_runner=$0 ;;
    *) invoked_runner=$(pwd -P)/$0 ;;
esac
[ ! -L "$invoked_runner" ] || fail "runner must not be invoked through a symlink"
invoked_runner_dir=$(CDPATH='' cd -- "$(dirname -- "$invoked_runner")" && pwd -P) ||
    fail "cannot resolve runner directory"
invoked_runner="$invoked_runner_dir/$(basename -- "$invoked_runner")"
[ "$invoked_runner" = "$runner_source" ] ||
    fail "runner must be invoked from its canonical repository path"

usage() {
    echo "usage: tools/ghidra/run-snapshot-bank.sh PROGRAM_SNAPSHOT BANK MATERIALIZED_BANK WORKSPACE BASE_SEED SNAPSHOT_SEED" >&2
    echo "       tools/ghidra/run-snapshot-bank.sh --unseeded-only PROGRAM_SNAPSHOT BANK MATERIALIZED_BANK WORKSPACE BASE_SEED" >&2
    echo "       tools/ghidra/run-snapshot-bank.sh --discovery-only PROGRAM_SNAPSHOT BANK MATERIALIZED_BANK WORKSPACE" >&2
    echo "requires: FN64_STAGE_SNAPSHOT_BANK, FN64_INGEST_TOOL_CLAIMS, GHIDRA_JAVA_HOME, and GHIDRA_INSTALL_DIR or GHIDRA_HEADLESS" >&2
    exit 2
}

require_absolute() {
    case "$2" in
        /*) ;;
        *) fail "$1 must be absolute" ;;
    esac
}

hash_file() {
    if command -v shasum >/dev/null 2>&1; then
        shasum -a 256 -- "$1" | awk '{print $1}'
    elif command -v sha256sum >/dev/null 2>&1; then
        sha256sum -- "$1" | awk '{print $1}'
    else
        fail "no SHA-256 utility is available"
    fi
}

file_mode() {
    stat -f '%Lp' "$1" 2>/dev/null || stat -c '%a' "$1" 2>/dev/null
}

file_owner() {
    stat -f '%u' "$1" 2>/dev/null || stat -c '%u' "$1" 2>/dev/null
}

execution_mode=paired
if [ "${1:-}" = --unseeded-only ]; then
    execution_mode=unseeded-only
    shift
elif [ "${1:-}" = --discovery-only ]; then
    execution_mode=discovery-only
    shift
elif [ "${1:-}" != "${1#--}" ]; then
    usage
fi
if [ "$execution_mode" = paired ]; then
    [ "$#" -eq 6 ] || usage
elif [ "$execution_mode" = unseeded-only ]; then
    [ "$#" -eq 5 ] || usage
elif [ "$execution_mode" = discovery-only ]; then
    [ "$#" -eq 4 ] || usage
fi
program_snapshot=$1
bank=$2
materialized_bank=$3
workspace=$4
base_seed=
snapshot_seed=
if [ "$execution_mode" = paired ]; then
    base_seed=$5
    snapshot_seed=$6
elif [ "$execution_mode" = unseeded-only ]; then
    base_seed=$5
fi

require_absolute PROGRAM_SNAPSHOT "$program_snapshot"
require_absolute MATERIALIZED_BANK "$materialized_bank"
require_absolute WORKSPACE "$workspace"
[ -f "$program_snapshot" ] && [ ! -L "$program_snapshot" ] ||
    fail "PROGRAM_SNAPSHOT must be a regular non-symlink file"
[ -f "$materialized_bank" ] && [ ! -L "$materialized_bank" ] ||
    fail "MATERIALIZED_BANK must be a regular non-symlink file"
[ -n "$bank" ] || fail "BANK must not be empty"

[ -d "$workspace" ] && [ ! -L "$workspace" ] ||
    fail "WORKSPACE must be an existing non-symlink directory"
resolved_workspace=$(CDPATH='' cd -- "$workspace" && pwd -P) || fail "cannot resolve WORKSPACE"
[ "$resolved_workspace" = "$workspace" ] ||
    fail "WORKSPACE must be absolute and canonical with no symlink traversal"
[ "$(file_mode "$workspace")" = 700 ] || fail "WORKSPACE must have mode 0700"
[ "$(file_owner "$workspace")" = "$(id -u)" ] || fail "WORKSPACE must be owned by the caller"
case "$workspace" in
    "$repo"|"$repo"/*) fail "WORKSPACE must be outside the repository" ;;
esac

stage=${FN64_STAGE_SNAPSHOT_BANK:-}
ingest=${FN64_INGEST_TOOL_CLAIMS:-}
for helper_spec in "FN64_STAGE_SNAPSHOT_BANK:$stage" "FN64_INGEST_TOOL_CLAIMS:$ingest"; do
    helper_name=${helper_spec%%:*}
    helper_path=${helper_spec#*:}
    [ -n "$helper_path" ] || fail "$helper_name is required"
    require_absolute "$helper_name" "$helper_path"
    [ -f "$helper_path" ] && [ -x "$helper_path" ] && [ ! -L "$helper_path" ] ||
        fail "$helper_name must be an executable regular non-symlink file"
done

jdk=${GHIDRA_JAVA_HOME:-}
[ -n "$jdk" ] || fail "GHIDRA_JAVA_HOME is required"
require_absolute GHIDRA_JAVA_HOME "$jdk"
[ -x "$jdk/bin/java" ] || fail "GHIDRA_JAVA_HOME does not contain bin/java"

if [ -n "${GHIDRA_INSTALL_DIR:-}" ]; then
    ghidra_install=$GHIDRA_INSTALL_DIR
    require_absolute GHIDRA_INSTALL_DIR "$ghidra_install"
    headless=${GHIDRA_HEADLESS:-$ghidra_install/support/analyzeHeadless}
else
    headless=${GHIDRA_HEADLESS:-}
    [ -n "$headless" ] || fail "GHIDRA_INSTALL_DIR or GHIDRA_HEADLESS is required"
    require_absolute GHIDRA_HEADLESS "$headless"
    ghidra_install=$(CDPATH='' cd -- "$(dirname -- "$headless")/.." && pwd -P) ||
        fail "could not derive GHIDRA_INSTALL_DIR from GHIDRA_HEADLESS"
fi
require_absolute GHIDRA_HEADLESS "$headless"
[ -x "$headless" ] || fail "GHIDRA_HEADLESS is not executable"
application_properties="$ghidra_install/Ghidra/application.properties"
[ -f "$application_properties" ] || fail "Ghidra application.properties is missing"
[ -x "$guard" ] || fail "repository memory guard is not executable"
[ -f "$runner_source" ] && [ -x "$runner_source" ] && [ ! -L "$runner_source" ] ||
    fail "repository snapshot-bank runner is not an executable regular non-symlink file"
[ -f "$guard" ] && [ ! -L "$guard" ] ||
    fail "repository memory guard must be a regular non-symlink file"
if [ "$execution_mode" != discovery-only ]; then
    [ -f "$seed_source" ] || fail "Fn64SeedFunctions.java is missing"
fi
[ -f "$export_source" ] || fail "Fn64ExportCandidates.java is missing"
[ -f "$distribution_manifest_source" ] && [ -x "$distribution_manifest_source" ] &&
    [ ! -L "$distribution_manifest_source" ] ||
    fail "Ghidra distribution manifest helper must be an executable regular non-symlink file"
command -v python3 >/dev/null 2>&1 || fail "python3 is required"

ghidra_version=$(awk -F= '$1 == "application.version" { print $2 }' "$application_properties")
[ -n "$ghidra_version" ] || fail "could not read Ghidra version"

attempt=$(mktemp -d "$workspace/ghidra-snapshot-bank.XXXXXXXX") ||
    fail "could not create private attempt"
chmod 700 "$attempt"
mkdir -m 700 "$attempt/inputs" "$attempt/raw" "$attempt/config" \
    "$attempt/tool-artifacts" "$attempt/diagnostics" "$attempt/out" "$attempt/modes"

bound_runner="$attempt/tool-artifacts/run-snapshot-bank.sh"
bound_guard="$attempt/tool-artifacts/memory-guard.zsh"
bound_distribution_manifest="$attempt/tool-artifacts/manifest-ghidra-distribution.py"
bound_stage="$attempt/tool-artifacts/stage_snapshot_bank"
bound_ingest="$attempt/tool-artifacts/ingest_tool_claims"
cp -- "$runner_source" "$bound_runner"
cp -- "$guard" "$bound_guard"
cp -- "$distribution_manifest_source" "$bound_distribution_manifest"
cp -- "$stage" "$bound_stage"
cp -- "$ingest" "$bound_ingest"
chmod 700 "$bound_guard" "$bound_distribution_manifest" "$bound_stage" "$bound_ingest"
chmod 600 "$bound_runner"

runner_sha=$(hash_file "$bound_runner")
guard_sha=$(hash_file "$bound_guard")
distribution_manifest_helper_sha=$(hash_file "$bound_distribution_manifest")
stage_sha=$(hash_file "$bound_stage")
ingest_sha=$(hash_file "$bound_ingest")
[ "$(hash_file "$runner_source")" = "$runner_sha" ] ||
    fail "snapshot-bank runner changed while copying"
[ "$(hash_file "$guard")" = "$guard_sha" ] ||
    fail "memory guard changed while copying"
[ "$(hash_file "$distribution_manifest_source")" = "$distribution_manifest_helper_sha" ] ||
    fail "distribution manifest helper changed while copying"
[ "$(hash_file "$stage")" = "$stage_sha" ] ||
    fail "stage helper changed while copying"
[ "$(hash_file "$ingest")" = "$ingest_sha" ] ||
    fail "ingest helper changed while copying"

verify_bound_artifact() {
    source_path=$1
    retained_path=$2
    expected_sha=$3
    label=$4
    [ -f "$source_path" ] && [ ! -L "$source_path" ] ||
        fail "$label source is no longer a regular non-symlink file"
    [ "$(hash_file "$source_path")" = "$expected_sha" ] ||
        fail "$label source changed during run"
    [ -f "$retained_path" ] && [ ! -L "$retained_path" ] ||
        fail "$label retained artifact is no longer a regular non-symlink file"
    [ "$(hash_file "$retained_path")" = "$expected_sha" ] ||
        fail "$label retained artifact changed during run"
}

active_guard_pid=
launching_guard=0
interrupted_signal=
idle_phase=before_distribution_scan

forward_runner_signal() {
    handler_signal=$1
    [ -n "$interrupted_signal" ] || interrupted_signal=$handler_signal
    if [ -z "$active_guard_pid" ] && [ "$launching_guard" -eq 0 ]; then
        finish_interruption "$idle_phase" 0
    fi
}

write_interruption_receipt() {
    receipt_signal=$1
    receipt_phase=$2
    receipt_guard_status=$3
    receipt="$attempt/diagnostics/runner-interruption.json"
    python3 - "$receipt" "$receipt_signal" "$receipt_phase" "$receipt_guard_status" <<'PY'
import json
import sys

path, signal_name, phase, guard_status = sys.argv[1:]
value = {
    "schema": "fn64.ghidra-snapshot-bank-interruption",
    "schema_version": 1,
    "signal": signal_name,
    "phase": phase,
    "guard_exit_status": int(guard_status),
    "active_guard_cleanup_complete": True,
}
with open(path, "x", encoding="utf-8") as stream:
    stream.write(json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n")
PY
    chmod 600 "$receipt"
}

finish_interruption() {
    interruption_phase=$1
    interruption_guard_status=$2
    trap '' HUP INT TERM
    write_interruption_receipt "$interrupted_signal" "$interruption_phase" \
        "$interruption_guard_status"
    finishing_signal=$interrupted_signal
    trap - HUP INT TERM
    case "$finishing_signal" in
        HUP) exit 129 ;;
        INT) exit 130 ;;
        TERM) exit 143 ;;
        *) exit 128 ;;
    esac
}

exit_if_interrupted() {
    checkpoint_phase=$1
    idle_phase=$checkpoint_phase
    if [ -n "$interrupted_signal" ]; then
        finish_interruption "$checkpoint_phase" 0
    fi
}

run_guarded_phase() {
    guard_phase=$1
    guard_jsonl=$2
    shift 2
    exit_if_interrupted "before_$guard_phase"
    guard_status=0
    termination_forwarded=0
    launching_guard=1
    FN64_GUARD_MAX_RSS_MIB=2048 \
    FN64_GUARD_MIN_FREE_PERCENT=40 \
    FN64_GUARD_MAX_SECONDS=180 \
    FN64_GUARD_JSONL="$guard_jsonl" \
        "$bound_guard" "$@" &
    active_guard_pid=$!
    launching_guard=0
    if [ -n "$interrupted_signal" ] && [ "$termination_forwarded" -eq 0 ]; then
        while [ ! -s "$guard_jsonl" ] && kill -0 "$active_guard_pid" 2>/dev/null; do
            sleep 0.01
        done
        if kill -0 "$active_guard_pid" 2>/dev/null; then
            kill -TERM "$active_guard_pid" 2>/dev/null || true
        fi
        termination_forwarded=1
    fi
    while :; do
        wait "$active_guard_pid"
        guard_status=$?
        if [ -n "$interrupted_signal" ] && [ "$termination_forwarded" -eq 0 ] &&
            kill -0 "$active_guard_pid" 2>/dev/null; then
            # The guard publishes its first sample only after its signal traps
            # and exact child process-group identity are installed. Forwarding
            # before this boundary could kill the guard during its launch
            # handshake and orphan the newly created process group.
            while [ ! -s "$guard_jsonl" ] && kill -0 "$active_guard_pid" 2>/dev/null; do
                sleep 0.01
            done
            if kill -0 "$active_guard_pid" 2>/dev/null; then
                kill -TERM "$active_guard_pid" 2>/dev/null || true
            fi
            termination_forwarded=1
        fi
        if kill -0 "$active_guard_pid" 2>/dev/null; then
            continue
        fi
        break
    done
    active_guard_pid=
    idle_phase="after_$guard_phase"
    [ -z "$interrupted_signal" ] || finish_interruption "$guard_phase" "$guard_status"
    return "$guard_status"
}

trap 'forward_runner_signal HUP' HUP
trap 'forward_runner_signal INT' INT
trap 'forward_runner_signal TERM' TERM

distribution_cache="$workspace/.fn64-ghidra-distribution-manifests"
if [ -e "$distribution_cache" ]; then
    [ -d "$distribution_cache" ] && [ ! -L "$distribution_cache" ] ||
        fail "Ghidra distribution manifest cache must be a non-symlink directory"
else
    mkdir -m 700 "$distribution_cache" || fail "could not create distribution manifest cache"
fi
[ "$(file_mode "$distribution_cache")" = 700 ] ||
    fail "Ghidra distribution manifest cache must have mode 0700"
[ "$(file_owner "$distribution_cache")" = "$(id -u)" ] ||
    fail "Ghidra distribution manifest cache must be owned by the caller"

distribution_manifest="$attempt/tool-artifacts/ghidra-distribution.json"
distribution_scan_log="$attempt/diagnostics/ghidra-distribution-scan.log"
distribution_scan_guard="$attempt/diagnostics/ghidra-distribution-scan-memory.jsonl"
path_value=${PATH:-/usr/bin:/bin}
set +e
run_guarded_phase distribution_scan "$distribution_scan_guard" \
    env -i "PATH=$path_value" "HOME=$attempt" "TMPDIR=$attempt" \
    "$bound_distribution_manifest" scan "$ghidra_install" "$distribution_cache" \
        "$distribution_manifest" >"$distribution_scan_log" 2>&1
distribution_scan_status=$?
set -e
[ "$distribution_scan_status" -eq 0 ] ||
    fail "Ghidra distribution inventory failed; see $distribution_scan_log"
[ -s "$distribution_manifest" ] || fail "Ghidra distribution inventory is empty"
grep -q '^ghidra-distribution-manifest: sha256=' "$distribution_scan_log" ||
    fail "Ghidra distribution inventory emitted no completion receipt"

snapshot_copy="$attempt/inputs/program-snapshot.json"
snapshot_source_sha=$(hash_file "$program_snapshot")
cp -- "$program_snapshot" "$snapshot_copy"
[ "$snapshot_source_sha" = "$(hash_file "$snapshot_copy")" ] ||
    fail "PROGRAM_SNAPSHOT changed while copying"
chmod 600 "$snapshot_copy"

staged_bank="$attempt/inputs/bank.bin"
evidence="$attempt/raw/evidence.json"
stage_log="$attempt/diagnostics/stage.log"
stage_guard="$attempt/diagnostics/stage-memory.jsonl"
set +e
if [ "$execution_mode" = paired ]; then
    run_guarded_phase stage "$stage_guard" \
        env -i "PATH=$path_value" "HOME=$attempt" "TMPDIR=$attempt" \
        "$bound_stage" "$snapshot_copy" "$bank" "$materialized_bank" "$workspace" \
            "$staged_bank" "$evidence" "$base_seed" "$snapshot_seed" \
            >"$stage_log" 2>&1
elif [ "$execution_mode" = unseeded-only ]; then
    run_guarded_phase stage "$stage_guard" \
        env -i "PATH=$path_value" "HOME=$attempt" "TMPDIR=$attempt" \
        "$bound_stage" --base-only "$snapshot_copy" "$bank" "$materialized_bank" \
            "$workspace" "$staged_bank" "$evidence" "$base_seed" \
            >"$stage_log" 2>&1
else
    run_guarded_phase stage "$stage_guard" \
        env -i "PATH=$path_value" "HOME=$attempt" "TMPDIR=$attempt" \
        "$bound_stage" --discovery-only "$snapshot_copy" "$bank" "$materialized_bank" \
            "$workspace" "$staged_bank" "$evidence" \
            >"$stage_log" 2>&1
fi
stage_status=$?
set -e
[ "$stage_status" -eq 0 ] || fail "snapshot-bank staging failed; see $stage_log"
[ -s "$staged_bank" ] || fail "stage_snapshot_bank produced no bank"
[ -s "$evidence" ] || fail "stage_snapshot_bank produced no evidence"
grep -q '^stage-snapshot-bank: snapshot=' "$stage_log" ||
    fail "stage_snapshot_bank emitted no completion receipt"

stage_fields="$attempt/diagnostics/stage-fields.txt"
python3 - "$evidence" "$bank" "$execution_mode" <<'PY' > "$stage_fields" ||
import json
import re
import sys

path, expected_bank, execution_mode = sys.argv[1:]
with open(path, "r", encoding="utf-8") as stream:
    value = json.load(stream)
if set(value) != {"schema", "schema_version", "program_snapshot_sha256", "input", "backing", "artifact", "seeds"}:
    raise SystemExit("wrong evidence fields")
expected_schema_version = 3 if execution_mode == "discovery-only" else 2
if value["schema"] != "fn64.snapshot-bank-evidence" or value["schema_version"] != expected_schema_version:
    raise SystemExit("wrong evidence schema")
if set(value["input"]) != {"normalized_rom_sha256", "bank", "bank_bytes_sha256", "mapping_sha256", "va_start", "va_end"}:
    raise SystemExit("wrong evidence input fields")
if set(value["backing"]) != {"rom_space", "rom_start", "rom_end"}:
    raise SystemExit("wrong evidence backing fields")
if set(value["artifact"]) != {"byte_length", "sha256"}:
    raise SystemExit("wrong evidence artifact fields")
if value["input"]["bank"] != expected_bank:
    raise SystemExit("wrong evidence bank")
for digest in (value["program_snapshot_sha256"], value["input"]["normalized_rom_sha256"],
               value["input"]["bank_bytes_sha256"], value["input"]["mapping_sha256"],
               value["artifact"]["sha256"]):
    if not isinstance(digest, str) or not re.fullmatch(r"[0-9a-f]{64}", digest):
        raise SystemExit("invalid evidence digest")
for field in (value["input"]["va_start"], value["input"]["va_end"],
              value["artifact"]["byte_length"]):
    if not isinstance(field, int) or isinstance(field, bool) or field < 0:
        raise SystemExit("invalid evidence integer")
seeds = value["seeds"]
if execution_mode == "paired":
    if set(seeds) != {"mode", "base_seed", "snapshot_seed"} or seeds["mode"] != "paired":
        raise SystemExit("wrong paired evidence seed fields")
    snapshot_seed = seeds["snapshot_seed"]
    if not isinstance(snapshot_seed, int) or isinstance(snapshot_seed, bool) or snapshot_seed < 0:
        raise SystemExit("paired evidence lacks a snapshot seed")
elif execution_mode == "unseeded-only":
    if set(seeds) != {"mode", "base_seed"} or seeds["mode"] != "base_only":
        raise SystemExit("wrong base-only evidence seed fields")
    snapshot_seed = None
elif execution_mode == "discovery-only":
    if seeds != {"mode": "discovery_only", "role": "candidate_only"}:
        raise SystemExit("wrong discovery-only evidence fields")
    base_seed = None
    snapshot_seed = None
else:
    raise SystemExit("invalid execution mode")
if execution_mode != "discovery-only":
    base_seed = seeds["base_seed"]
    if not isinstance(base_seed, int) or isinstance(base_seed, bool) or base_seed < 0:
        raise SystemExit("invalid evidence base seed")
print(value["program_snapshot_sha256"])
print(value["input"]["normalized_rom_sha256"])
print(value["input"]["bank_bytes_sha256"])
print(value["input"]["mapping_sha256"])
print(value["input"]["va_start"])
print(value["input"]["va_end"])
print(value["artifact"]["byte_length"])
print(value["artifact"]["sha256"])
print("none" if base_seed is None else base_seed)
print("none" if snapshot_seed is None else snapshot_seed)
PY
    fail "stage_snapshot_bank produced invalid evidence"

snapshot_sha=$(sed -n '1p' "$stage_fields")
rom_sha=$(sed -n '2p' "$stage_fields")
bank_sha=$(sed -n '3p' "$stage_fields")
mapping_sha=$(sed -n '4p' "$stage_fields")
va_start=$(sed -n '5p' "$stage_fields")
va_end=$(sed -n '6p' "$stage_fields")
bank_length=$(sed -n '7p' "$stage_fields")
artifact_sha=$(sed -n '8p' "$stage_fields")
base_seed_decimal=$(sed -n '9p' "$stage_fields")
snapshot_seed_decimal=$(sed -n '10p' "$stage_fields")
[ "$bank_sha" = "$artifact_sha" ] || fail "staged artifact and bank digests disagree"
[ "$bank_sha" = "$(hash_file "$staged_bank")" ] || fail "staged bank digest is wrong"
[ "$bank_length" -eq "$(wc -c < "$staged_bank" | tr -d ' ')" ] ||
    fail "staged bank length is wrong"

cp -- "$export_source" "$attempt/tool-artifacts/Fn64ExportCandidates.java"
if [ "$execution_mode" != discovery-only ]; then
    cp -- "$seed_source" "$attempt/tool-artifacts/Fn64SeedFunctions.java"
fi
cp -- "$headless" "$attempt/tool-artifacts/analyzeHeadless"
cp -- "$application_properties" "$attempt/tool-artifacts/application.properties"
cp -- "$jdk/bin/java" "$attempt/tool-artifacts/java"
[ "$(hash_file "$export_source")" = "$(hash_file "$attempt/tool-artifacts/Fn64ExportCandidates.java")" ] ||
    fail "export script changed while copying"
if [ "$execution_mode" != discovery-only ]; then
    [ "$(hash_file "$seed_source")" = "$(hash_file "$attempt/tool-artifacts/Fn64SeedFunctions.java")" ] ||
        fail "seed script changed while copying"
fi
[ "$(hash_file "$headless")" = "$(hash_file "$attempt/tool-artifacts/analyzeHeadless")" ] ||
    fail "Ghidra launcher changed while copying"
[ "$(hash_file "$application_properties")" = "$(hash_file "$attempt/tool-artifacts/application.properties")" ] ||
    fail "Ghidra properties changed while copying"
[ "$(hash_file "$jdk/bin/java")" = "$(hash_file "$attempt/tool-artifacts/java")" ] ||
    fail "Java executable changed while copying"
chmod 600 "$attempt/tool-artifacts/Fn64ExportCandidates.java" \
    "$attempt/tool-artifacts/analyzeHeadless" \
    "$attempt/tool-artifacts/application.properties" \
    "$attempt/tool-artifacts/ghidra-distribution.json" \
    "$attempt/tool-artifacts/java"
if [ "$execution_mode" != discovery-only ]; then
    chmod 600 "$attempt/tool-artifacts/Fn64SeedFunctions.java"
fi

export_size=$(wc -c < "$attempt/tool-artifacts/Fn64ExportCandidates.java" | tr -d ' ')
export_sha=$(hash_file "$attempt/tool-artifacts/Fn64ExportCandidates.java")
seed_size=
seed_sha=
if [ "$execution_mode" != discovery-only ]; then
    seed_size=$(wc -c < "$attempt/tool-artifacts/Fn64SeedFunctions.java" | tr -d ' ')
    seed_sha=$(hash_file "$attempt/tool-artifacts/Fn64SeedFunctions.java")
fi
headless_size=$(wc -c < "$attempt/tool-artifacts/analyzeHeadless" | tr -d ' ')
headless_sha=$(hash_file "$attempt/tool-artifacts/analyzeHeadless")
properties_size=$(wc -c < "$attempt/tool-artifacts/application.properties" | tr -d ' ')
properties_sha=$(hash_file "$attempt/tool-artifacts/application.properties")
java_size=$(wc -c < "$attempt/tool-artifacts/java" | tr -d ' ')
java_sha=$(hash_file "$attempt/tool-artifacts/java")
distribution_size=$(wc -c < "$distribution_manifest" | tr -d ' ')
distribution_sha=$(hash_file "$distribution_manifest")
ingest_size=$(wc -c < "$bound_ingest" | tr -d ' ')
manifest_helper_size=$(wc -c < "$bound_distribution_manifest" | tr -d ' ')
guard_size=$(wc -c < "$bound_guard" | tr -d ' ')
runner_size=$(wc -c < "$bound_runner" | tr -d ' ')
stage_size=$(wc -c < "$bound_stage" | tr -d ' ')
orchestration_manifest="$attempt/tool-artifacts/orchestration.json"
python3 - "$orchestration_manifest" \
    "$ingest_size" "$ingest_sha" \
    "$manifest_helper_size" "$distribution_manifest_helper_sha" \
    "$guard_size" "$guard_sha" "$runner_size" "$runner_sha" \
    "$stage_size" "$stage_sha" <<'PY'
import json
import sys

(path, ingest_size, ingest_sha, manifest_helper_size, manifest_helper_sha,
 guard_size, guard_sha, runner_size, runner_sha, stage_size, stage_sha) = sys.argv[1:]
value = {
    "schema": "fn64.ghidra-orchestration-artifacts",
    "schema_version": 1,
    "artifacts": [
        {"path": "tool-artifacts/ingest_tool_claims", "byte_length": int(ingest_size), "sha256": ingest_sha},
        {"path": "tool-artifacts/manifest-ghidra-distribution.py", "byte_length": int(manifest_helper_size), "sha256": manifest_helper_sha},
        {"path": "tool-artifacts/memory-guard.zsh", "byte_length": int(guard_size), "sha256": guard_sha},
        {"path": "tool-artifacts/run-snapshot-bank.sh", "byte_length": int(runner_size), "sha256": runner_sha},
        {"path": "tool-artifacts/stage_snapshot_bank", "byte_length": int(stage_size), "sha256": stage_sha},
    ],
}
with open(path, "x", encoding="utf-8") as stream:
    stream.write(json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n")
PY
chmod 600 "$orchestration_manifest"
orchestration_size=$(wc -c < "$orchestration_manifest" | tr -d ' ')
orchestration_sha=$(hash_file "$orchestration_manifest")
distribution_file_count=$(python3 - "$distribution_manifest" <<'PY'
import json
import sys

with open(sys.argv[1], "r", encoding="utf-8") as stream:
    value = json.load(stream)
if set(value) != {"schema", "schema_version", "files"}:
    raise SystemExit("wrong Ghidra distribution manifest fields")
if value["schema"] != "fn64.ghidra-distribution-manifest" or value["schema_version"] != 1:
    raise SystemExit("wrong Ghidra distribution manifest schema")
if not isinstance(value["files"], list) or not value["files"]:
    raise SystemExit("empty Ghidra distribution manifest")
print(len(value["files"]))
PY
) || fail "Ghidra distribution inventory is invalid"

make_tool_manifest() {
    mode=$1
    manifest=$2
    python3 - "$manifest" "$mode" "$execution_mode" "$ghidra_version" \
        "$export_size" "$export_sha" "$seed_size" "$seed_sha" \
        "$headless_size" "$headless_sha" "$properties_size" "$properties_sha" \
        "$distribution_size" "$distribution_sha" "$java_size" "$java_sha" \
        "$orchestration_size" "$orchestration_sha" <<'PY'
import json
import sys

(path, mode, execution_mode, version, export_size, export_sha, seed_size, seed_sha,
 headless_size, headless_sha, properties_size, properties_sha,
 distribution_size, distribution_sha, java_size, java_sha,
 orchestration_size, orchestration_sha) = sys.argv[1:]
value = {
    "schema": "fn64.tool-artifact-manifest",
    "schema_version": 1,
    "tool_name": "ghidra-headless-" + mode,
    "tool_version": version,
    "artifacts": [
        {"path": "tool-artifacts/Fn64ExportCandidates.java", "byte_length": int(export_size), "sha256": export_sha},
        {"path": "tool-artifacts/analyzeHeadless", "byte_length": int(headless_size), "sha256": headless_sha},
        {"path": "tool-artifacts/application.properties", "byte_length": int(properties_size), "sha256": properties_sha},
        {"path": "tool-artifacts/ghidra-distribution.json", "byte_length": int(distribution_size), "sha256": distribution_sha},
        {"path": "tool-artifacts/java", "byte_length": int(java_size), "sha256": java_sha},
        {"path": "tool-artifacts/orchestration.json", "byte_length": int(orchestration_size), "sha256": orchestration_sha},
    ],
}
if execution_mode != "discovery-only":
    value["artifacts"].insert(1, {"path": "tool-artifacts/Fn64SeedFunctions.java", "byte_length": int(seed_size), "sha256": seed_sha})
with open(path, "x", encoding="utf-8") as stream:
    stream.write(json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n")
PY
}

unseeded_tool_manifest="$attempt/tool-unseeded.json"
seeded_tool_manifest="$attempt/tool-seeded.json"
make_tool_manifest unseeded "$unseeded_tool_manifest"
unseeded_tool_sha=$(hash_file "$unseeded_tool_manifest")
seeded_tool_sha=
if [ "$execution_mode" = paired ]; then
    make_tool_manifest seeded "$seeded_tool_manifest"
    seeded_tool_sha=$(hash_file "$seeded_tool_manifest")
fi

make_config() {
    mode=$1
    tool_sha=$2
    config=$3
    mode_snapshot_seed=$4
    python3 - "$config" "$mode" "$execution_mode" "$bank" "$va_start" "$va_end" \
        "$base_seed_decimal" "$mode_snapshot_seed" "$ghidra_version" "$tool_sha" <<'PY'
import json
import sys

path, mode, execution_mode, bank, va_start, va_end, base_seed, snapshot_seed, version, tool_sha = sys.argv[1:]
value = {
    "schema": "fn64.ghidra-bank-config",
    "schema_version": 1,
    "mode": mode,
    "bank": bank,
    "va_start": int(va_start),
    "va_end": int(va_end),
    "base_seed": None if base_seed == "none" else int(base_seed),
    "snapshot_seed": None if snapshot_seed == "none" else int(snapshot_seed),
    "loader": "BinaryLoader",
    "processor": "MIPS:BE:64:64-32addr",
    "cspec": "o32",
    "ghidra_version": version,
    "analysis_timeout_seconds": 120,
    "max_cpu": 1,
    "heap_mib": 1024,
    "rss_mib": 2048,
    "min_free_percent": 40,
    "wall_seconds": 180,
    "tool_manifest_sha256": tool_sha,
}
if execution_mode == "discovery-only":
    if mode != "unseeded" or value["base_seed"] is not None or value["snapshot_seed"] is not None:
        raise SystemExit("discovery-only config unexpectedly binds a seed")
    value["role"] = "candidate_only"
with open(path, "x", encoding="utf-8") as stream:
    stream.write(json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n")
PY
}

unseeded_config="$attempt/config/unseeded.json"
seeded_config="$attempt/config/seeded.json"
make_config unseeded "$unseeded_tool_sha" "$unseeded_config" none
unseeded_config_sha=$(hash_file "$unseeded_config")
seeded_config_sha=
if [ "$execution_mode" = paired ]; then
    make_config seeded "$seeded_tool_sha" "$seeded_config" "$snapshot_seed_decimal"
    seeded_config_sha=$(hash_file "$seeded_config")
fi
evidence_sha=$(hash_file "$evidence")
va_start_arg=$(printf '0x%08x' "$va_start")
va_end_arg=$(printf '0x%08x' "$va_end")
loader_base_arg=$(printf '%08x' "$va_start")
base_seed_arg=
if [ "$execution_mode" != discovery-only ]; then
    base_seed_arg=$(printf '0x%08x' "$base_seed_decimal")
fi
snapshot_seed_arg=
if [ "$execution_mode" = paired ]; then
    snapshot_seed_arg=$(printf '0x%08x' "$snapshot_seed_decimal")
fi
program_name=$(basename -- "$staged_bank")
ghidra_executable_ranges=${FN64_GHIDRA_EXECUTABLE_RANGES:-0}
case "$ghidra_executable_ranges" in
    0|1) ;;
    *) fail "FN64_GHIDRA_EXECUTABLE_RANGES must be 0 or 1" ;;
esac
if [ "$ghidra_executable_ranges" = 1 ]; then
    tool_claim_role=region_candidates
else
    tool_claim_role=function_boundary_candidates
fi

run_mode() {
    mode=$1
    tool_sha=$2
    config_sha=$3
    mode_root="$attempt/modes/$mode"
    mkdir -m 700 "$mode_root" "$mode_root/project" "$mode_root/home" "$mode_root/tmp" \
        "$mode_root/cache" "$mode_root/settings" "$mode_root/out" "$mode_root/diagnostics"
    jsonl="$mode_root/out/provider.jsonl"
    analysis_log="$mode_root/diagnostics/analyze.log"
    analysis_guard="$mode_root/diagnostics/memory.jsonl"

    set +e
    if [ "$mode" = seeded ]; then
        run_guarded_phase "analysis_$mode" "$analysis_guard" \
            env -i \
            "PATH=$path_value" "HOME=$mode_root/home" "TMPDIR=$mode_root/tmp" \
            "JAVA_HOME=$jdk" "GHIDRA_HEADLESS_MAXMEM=1G" \
            "FN64_GHIDRA_EXECUTABLE_RANGES=$ghidra_executable_ranges" \
            "_JAVA_OPTIONS=-Dapplication.settingsdir=$mode_root/settings -Dapplication.cachedir=$mode_root/cache -Dapplication.tempdir=$mode_root/tmp -Djava.io.tmpdir=$mode_root/tmp -Duser.home=$mode_root/home" \
            "$headless" "$mode_root/project" "snapshot-bank-$mode" \
                -import "$staged_bank" -overwrite \
                -processor MIPS:BE:64:64-32addr -cspec o32 \
                -loader BinaryLoader -loader-baseAddr "$loader_base_arg" \
                -scriptPath "$attempt/tool-artifacts" \
                -preScript Fn64SeedFunctions.java seeded "$va_start_arg" "$va_end_arg" \
                    "$base_seed_arg" "$snapshot_seed_arg" \
                -analysisTimeoutPerFile 120 -max-cpu 1 \
                -postScript Fn64ExportCandidates.java "$jsonl" seeded "$bank" \
                    "$va_start_arg" "$va_end_arg" "$rom_sha" "$bank_sha" "$mapping_sha" \
                    "$ghidra_version" "$tool_sha" "$config_sha" "$evidence_sha" \
                    "$program_name" discovery_snapshot "$snapshot_sha" \
                -deleteProject >"$analysis_log" 2>&1
    elif [ "$execution_mode" != discovery-only ]; then
        run_guarded_phase "analysis_$mode" "$analysis_guard" \
            env -i \
            "PATH=$path_value" "HOME=$mode_root/home" "TMPDIR=$mode_root/tmp" \
            "JAVA_HOME=$jdk" "GHIDRA_HEADLESS_MAXMEM=1G" \
            "FN64_GHIDRA_EXECUTABLE_RANGES=$ghidra_executable_ranges" \
            "_JAVA_OPTIONS=-Dapplication.settingsdir=$mode_root/settings -Dapplication.cachedir=$mode_root/cache -Dapplication.tempdir=$mode_root/tmp -Djava.io.tmpdir=$mode_root/tmp -Duser.home=$mode_root/home" \
            "$headless" "$mode_root/project" "snapshot-bank-$mode" \
                -import "$staged_bank" -overwrite \
                -processor MIPS:BE:64:64-32addr -cspec o32 \
                -loader BinaryLoader -loader-baseAddr "$loader_base_arg" \
                -scriptPath "$attempt/tool-artifacts" \
                -preScript Fn64SeedFunctions.java unseeded "$va_start_arg" "$va_end_arg" \
                    "$base_seed_arg" \
                -analysisTimeoutPerFile 120 -max-cpu 1 \
                -postScript Fn64ExportCandidates.java "$jsonl" unseeded "$bank" \
                    "$va_start_arg" "$va_end_arg" "$rom_sha" "$bank_sha" "$mapping_sha" \
                    "$ghidra_version" "$tool_sha" "$config_sha" "$evidence_sha" \
                    "$program_name" discovery_snapshot "$snapshot_sha" \
                -deleteProject >"$analysis_log" 2>&1
    else
        run_guarded_phase "analysis_$mode" "$analysis_guard" \
            env -i \
            "PATH=$path_value" "HOME=$mode_root/home" "TMPDIR=$mode_root/tmp" \
            "JAVA_HOME=$jdk" "GHIDRA_HEADLESS_MAXMEM=1G" \
            "FN64_GHIDRA_EXECUTABLE_RANGES=$ghidra_executable_ranges" \
            "_JAVA_OPTIONS=-Dapplication.settingsdir=$mode_root/settings -Dapplication.cachedir=$mode_root/cache -Dapplication.tempdir=$mode_root/tmp -Djava.io.tmpdir=$mode_root/tmp -Duser.home=$mode_root/home" \
            "$headless" "$mode_root/project" "snapshot-bank-$mode" \
                -import "$staged_bank" -overwrite \
                -processor MIPS:BE:64:64-32addr -cspec o32 \
                -loader BinaryLoader -loader-baseAddr "$loader_base_arg" \
                -scriptPath "$attempt/tool-artifacts" \
                -analysisTimeoutPerFile 120 -max-cpu 1 \
                -postScript Fn64ExportCandidates.java "$jsonl" unseeded "$bank" \
                    "$va_start_arg" "$va_end_arg" "$rom_sha" "$bank_sha" "$mapping_sha" \
                    "$ghidra_version" "$tool_sha" "$config_sha" "$evidence_sha" \
                    "$program_name" discovery_snapshot "$snapshot_sha" \
                -deleteProject >"$analysis_log" 2>&1
    fi
    analysis_status=$?
    set -e
    [ "$analysis_status" -eq 0 ] || fail "$mode Ghidra analysis failed; see $analysis_log"
    [ -s "$jsonl" ] || fail "$mode Ghidra analysis produced no provider JSONL"
    grep -q 'Using Loader: Raw Binary' "$analysis_log" ||
        fail "$mode analysis did not use stock BinaryLoader"
    grep -q 'Using Language/Compiler: MIPS:BE:64:64-32addr:o32' "$analysis_log" ||
        fail "$mode analysis did not use the pinned MIPS/o32 language"
    if grep -q 'Using Loader: N64 Loader by Warranty Voider' "$analysis_log"; then
        fail "$mode analysis unexpectedly used N64LoaderWV"
    fi
    if find "$mode_root/project" -mindepth 1 -print -quit | grep -q .; then
        fail "$mode Ghidra project was not deleted"
    fi
}

run_mode unseeded "$unseeded_tool_sha" "$unseeded_config_sha"
if [ "$execution_mode" = paired ]; then
    run_mode seeded "$seeded_tool_sha" "$seeded_config_sha"
fi

[ "$(hash_file "$application_properties")" = "$properties_sha" ] ||
    fail "Ghidra application properties changed during analysis"
[ "$(hash_file "$export_source")" = "$export_sha" ] ||
    fail "repository export script changed during analysis"
if [ "$execution_mode" != discovery-only ]; then
    [ "$(hash_file "$seed_source")" = "$seed_sha" ] ||
        fail "repository seed script changed during analysis"
fi
[ "$(hash_file "$headless")" = "$headless_sha" ] ||
    fail "Ghidra launcher changed during analysis"
[ "$(hash_file "$jdk/bin/java")" = "$java_sha" ] ||
    fail "Java executable changed during analysis"

distribution_verify_log="$attempt/diagnostics/ghidra-distribution-verify.log"
distribution_verify_guard="$attempt/diagnostics/ghidra-distribution-verify-memory.jsonl"
set +e
run_guarded_phase distribution_verify "$distribution_verify_guard" \
    env -i "PATH=$path_value" "HOME=$attempt" "TMPDIR=$attempt" \
    "$bound_distribution_manifest" verify "$ghidra_install" \
        "$distribution_manifest" >"$distribution_verify_log" 2>&1
distribution_verify_status=$?
set -e
[ "$distribution_verify_status" -eq 0 ] ||
    fail "Ghidra distribution changed during analysis; see $distribution_verify_log"
grep -q '^ghidra-distribution-manifest: verified sha256=' "$distribution_verify_log" ||
    fail "Ghidra distribution verification emitted no completion receipt"
[ "$(hash_file "$distribution_manifest")" = "$distribution_sha" ] ||
    fail "Ghidra distribution inventory changed during analysis"

request="$attempt/request.json"
python3 - "$request" "$execution_mode" "$bank" "$ghidra_version" \
    "$unseeded_tool_sha" "$seeded_tool_sha" "$tool_claim_role" <<'PY'
import json
import sys

path, execution_mode, bank, version, unseeded_sha, seeded_sha, role = sys.argv[1:]
def run(mode, tool_sha):
    return {
        "bank": bank,
        "jsonl": f"modes/{mode}/out/provider.jsonl",
        "tool": {"name": f"ghidra-headless-{mode}", "version": version, "build_sha256": tool_sha},
        "tool_artifact_manifest": f"tool-{mode}.json",
        "role": role,
        "lineage_artifacts": [
            {"role": "tool_configuration", "path": f"config/{mode}.json"},
            {"role": "evidence_manifest", "path": "raw/evidence.json"},
        ],
    }
runs = [run("unseeded", unseeded_sha)]
if execution_mode == "paired":
    runs.append(run("seeded", seeded_sha))
elif execution_mode not in {"unseeded-only", "discovery-only"}:
    raise SystemExit("invalid execution mode")
value = {"schema": "fn64.tool-ingest-request", "schema_version": 1, "runs": runs}
with open(path, "x", encoding="utf-8") as stream:
    stream.write(json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n")
PY

claims="$attempt/out/tool-claims.json"
ingest_log="$attempt/diagnostics/ingest.log"
ingest_guard="$attempt/diagnostics/ingest-memory.jsonl"
set +e
run_guarded_phase ingest "$ingest_guard" \
    env -i "PATH=$path_value" "HOME=$attempt" "TMPDIR=$attempt" \
    "$bound_ingest" "$snapshot_copy" "$request" "$workspace" "$claims" \
        >"$ingest_log" 2>&1
ingest_status=$?
set -e
[ "$ingest_status" -eq 0 ] || fail "strict tool-claim ingest failed; see $ingest_log"
[ -s "$claims" ] || fail "ingest_tool_claims produced no sidecar"
grep -q '^ingest-tool-claims: snapshot=' "$ingest_log" ||
    fail "ingest_tool_claims emitted no completion receipt"
python3 - "$claims" "$snapshot_sha" "$execution_mode" <<'PY' ||
import json
import sys

path, snapshot_sha, execution_mode = sys.argv[1:]
with open(path, "r", encoding="utf-8") as stream:
    value = json.load(stream)
if value.get("schema") != "fn64.tool-claim-set" or value.get("schema_version") != 1:
    raise SystemExit("wrong tool-claims schema")
if value.get("program_snapshot_sha256") != snapshot_sha:
    raise SystemExit("wrong tool-claims snapshot")
sources = value.get("sources")
expected_names = ["ghidra-headless-unseeded"]
if execution_mode == "paired":
    expected_names.append("ghidra-headless-seeded")
elif execution_mode not in {"unseeded-only", "discovery-only"}:
    raise SystemExit("invalid execution mode")
if not isinstance(sources, list) or len(sources) != len(expected_names):
    raise SystemExit("tool-claims sidecar contains the wrong source count")
names = sorted(source.get("tool", {}).get("name") for source in sources)
if names != sorted(expected_names):
    raise SystemExit("tool-claims sidecar contains wrong sources")
if not isinstance(value.get("claims"), list) or not value["claims"]:
    raise SystemExit("tool-claims sidecar contains no claims")
PY
    fail "ingest_tool_claims produced an invalid sidecar"
exit_if_interrupted after_ingest_validation

verify_bound_artifact "$runner_source" "$bound_runner" "$runner_sha" \
    "snapshot-bank runner"
verify_bound_artifact "$guard" "$bound_guard" "$guard_sha" "memory guard"
verify_bound_artifact "$distribution_manifest_source" "$bound_distribution_manifest" \
    "$distribution_manifest_helper_sha" "distribution manifest helper"
verify_bound_artifact "$stage" "$bound_stage" "$stage_sha" "stage helper"
verify_bound_artifact "$ingest" "$bound_ingest" "$ingest_sha" "ingest helper"
[ "$(hash_file "$orchestration_manifest")" = "$orchestration_sha" ] ||
    fail "orchestration identity manifest changed during run"

unseeded_provider="$attempt/modes/unseeded/out/provider.jsonl"
seeded_provider="$attempt/modes/seeded/out/provider.jsonl"
unseeded_guard="$attempt/modes/unseeded/diagnostics/memory.jsonl"
seeded_guard="$attempt/modes/seeded/diagnostics/memory.jsonl"
for retained in "$distribution_manifest" "$distribution_scan_log" \
        "$distribution_scan_guard" "$distribution_verify_log" \
        "$distribution_verify_guard" "$stage_guard" "$unseeded_guard" \
        "$ingest_guard" "$unseeded_config" "$unseeded_provider"; do
    [ -s "$retained" ] || fail "required retained artifact is empty: $retained"
done
if [ "$execution_mode" = paired ]; then
    for retained in "$seeded_guard" "$seeded_config" "$seeded_provider"; do
        [ -s "$retained" ] || fail "required retained artifact is empty: $retained"
    done
fi

receipt="$attempt/out/receipt.json"
seeded_provider_sha=
seeded_guard_sha=
if [ "$execution_mode" = paired ]; then
    seeded_provider_sha=$(hash_file "$seeded_provider")
    seeded_guard_sha=$(hash_file "$seeded_guard")
fi
exit_if_interrupted before_receipt
python3 - "$receipt" "$execution_mode" "$snapshot_sha" "$bank" \
    "$base_seed_decimal" "$snapshot_seed_decimal" "$evidence_sha" \
    "$(hash_file "$request")" "$unseeded_tool_sha" "$seeded_tool_sha" \
    "$(hash_file "$claims")" "$unseeded_config_sha" "$seeded_config_sha" \
    "$(hash_file "$unseeded_provider")" "$seeded_provider_sha" \
    "$distribution_sha" "$distribution_file_count" \
    "$(hash_file "$distribution_scan_log")" \
    "$(hash_file "$distribution_scan_guard")" \
    "$(hash_file "$distribution_verify_log")" \
    "$(hash_file "$distribution_verify_guard")" \
    "$(hash_file "$stage_guard")" "$(hash_file "$unseeded_guard")" \
    "$seeded_guard_sha" "$(hash_file "$ingest_guard")" <<'PY'
import json
import sys

(path, execution_mode, snapshot_sha, bank, base_seed, snapshot_seed, evidence_sha,
 request_sha, unseeded_tool_sha,
 seeded_tool_sha, claims_sha, unseeded_config_sha, seeded_config_sha,
 unseeded_provider_sha, seeded_provider_sha, distribution_sha,
 distribution_file_count, distribution_scan_log_sha,
 distribution_scan_guard_sha, distribution_verify_log_sha,
 distribution_verify_guard_sha, stage_guard_sha,
 unseeded_guard_sha, seeded_guard_sha, ingest_guard_sha) = sys.argv[1:]
paired = execution_mode == "paired"
discovery_only = execution_mode == "discovery-only"
if not paired and not discovery_only and execution_mode != "unseeded-only":
    raise SystemExit("invalid execution mode")
value = {
    "schema": "fn64.ghidra-snapshot-bank-receipt",
    "schema_version": 1,
    "execution_mode": execution_mode,
    "paired_comparison_complete": paired,
    "completed_modes": ["unseeded", "seeded"] if paired else ["unseeded"],
    "program_snapshot_sha256": snapshot_sha,
    "bank": bank,
    "seeds": (
        {"mode": "paired", "base_seed": int(base_seed), "snapshot_seed": int(snapshot_seed)}
        if paired else
        ({"mode": "discovery_only", "role": "candidate_only"}
         if discovery_only else
         {"mode": "base_only", "base_seed": int(base_seed)})
    ),
    "evidence_sha256": evidence_sha,
    "request_sha256": request_sha,
    "unseeded_tool_manifest_sha256": unseeded_tool_sha,
    "tool_claims_sha256": claims_sha,
    "ghidra_distribution_manifest_complete": True,
    "ghidra_distribution_manifest_sha256": distribution_sha,
    "ghidra_distribution_file_count": int(distribution_file_count),
    "tool_artifact_scope": "all-ghidra-install-regular-files,jdk-java,fn64-analysis-scripts,and-bound-orchestration-helpers",
    "configuration_sha256": {"unseeded": unseeded_config_sha},
    "provider_jsonl_sha256": {"unseeded": unseeded_provider_sha},
    "resource_evidence_sha256": {
        "ghidra_distribution_scan_log": distribution_scan_log_sha,
        "ghidra_distribution_scan": distribution_scan_guard_sha,
        "ghidra_distribution_verify_log": distribution_verify_log_sha,
        "ghidra_distribution_verify": distribution_verify_guard_sha,
        "stage": stage_guard_sha,
        "unseeded": unseeded_guard_sha,
        "ingest": ingest_guard_sha,
    },
}
if paired:
    value["seeded_tool_manifest_sha256"] = seeded_tool_sha
    value["configuration_sha256"]["seeded"] = seeded_config_sha
    value["provider_jsonl_sha256"]["seeded"] = seeded_provider_sha
    value["resource_evidence_sha256"]["seeded"] = seeded_guard_sha
with open(path, "x", encoding="utf-8") as stream:
    stream.write(json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n")
PY

chmod 600 "$evidence" "$stage_log" "$stage_guard" "$stage_fields" \
    "$distribution_manifest" "$distribution_scan_log" "$distribution_scan_guard" \
    "$distribution_verify_log" "$distribution_verify_guard" \
    "$unseeded_tool_manifest" "$unseeded_config" "$request" "$claims" "$receipt" \
    "$ingest_log" "$ingest_guard" \
    "$attempt/modes/unseeded/out/provider.jsonl" \
    "$attempt/modes/unseeded/diagnostics/analyze.log" \
    "$attempt/modes/unseeded/diagnostics/memory.jsonl"
chmod 600 "$attempt/tool-artifacts/"*
if [ "$execution_mode" = paired ]; then
    chmod 600 "$seeded_tool_manifest" "$seeded_config" \
        "$attempt/modes/seeded/out/provider.jsonl" \
        "$attempt/modes/seeded/diagnostics/analyze.log" \
        "$attempt/modes/seeded/diagnostics/memory.jsonl"
fi

echo "ghidra snapshot-bank: complete"
echo "execution_mode=$execution_mode"
echo "attempt=$attempt"
echo "request=$request"
echo "tool_claims=$claims"
echo "evidence=$evidence"
echo "receipt=$receipt"
echo "diagnostics=$attempt/diagnostics"
