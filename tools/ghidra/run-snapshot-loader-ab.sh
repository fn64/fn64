#!/bin/sh
set -eu
umask 077

repo=$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd -P)
runner_source="$repo/tools/ghidra/run-snapshot-loader-ab.sh"
guard_source="$repo/scripts/memory-guard.zsh"
stage_source=${FN64_STAGE_SNAPSHOT_BANK:-}
manifest_source="$repo/tools/ghidra/manifest-ghidra-distribution.py"
policy_source="$repo/tools/ghidra/n64loaderwv-source-policy.json"
artifact_policy_source="$repo/tools/ghidra/n64loaderwv-artifact-policy.json"
verifier_source="$repo/tools/ghidra/verify-n64loaderwv-provenance.py"
launcher_verifier_source="$repo/tools/ghidra/verify-ghidra-launcher.py"
install_verifier_source="$repo/tools/ghidra/verify-n64loaderwv-install.py"
runtime_verifier_source="$repo/tools/ghidra/Fn64VerifyN64LoaderRuntime.java"
exporter_source="$repo/tools/ghidra/Fn64ExportLoaderComparison.java"
comparator_source="$repo/tools/ghidra/compare-snapshot-loader-ab.py"

fail() {
    echo "ghidra snapshot-loader-ab: $1" >&2
    exit 2
}

usage() {
    echo "usage: tools/ghidra/run-snapshot-loader-ab.sh PROGRAM_SNAPSHOT BANK MATERIALIZED_BANK WORKSPACE EXTENSION_ZIP CONFORMANCE_RECEIPT" >&2
    echo "requires: FN64_STAGE_SNAPSHOT_BANK, GHIDRA_JAVA_HOME, and GHIDRA_INSTALL_DIR or GHIDRA_HEADLESS" >&2
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

hash_fields() {
    printf '%s\n' "$@" | {
        if command -v shasum >/dev/null 2>&1; then
            shasum -a 256 | awk '{print $1}'
        elif command -v sha256sum >/dev/null 2>&1; then
            sha256sum | awk '{print $1}'
        else
            fail "no SHA-256 utility is available"
        fi
    }
}

file_mode() {
    stat -c '%a' "$1" 2>/dev/null || stat -f '%Lp' "$1" 2>/dev/null
}

file_owner() {
    stat -c '%u' "$1" 2>/dev/null || stat -f '%u' "$1" 2>/dev/null
}

[ "$#" -eq 6 ] || usage
program_snapshot=$1
bank=$2
materialized_bank=$3
workspace=$4
extension_zip=$5
conformance_receipt=$6

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

for input_spec in \
        "PROGRAM_SNAPSHOT:$program_snapshot" \
        "MATERIALIZED_BANK:$materialized_bank" \
        "EXTENSION_ZIP:$extension_zip" \
        "CONFORMANCE_RECEIPT:$conformance_receipt"; do
    input_name=${input_spec%%:*}
    input_path=${input_spec#*:}
    require_absolute "$input_name" "$input_path"
    [ -f "$input_path" ] && [ ! -L "$input_path" ] ||
        fail "$input_name must be a regular non-symlink file"
done
case "$bank" in
    ''|.|..|*[!A-Za-z0-9._+-]*) fail "BANK must be a path-free logical token" ;;
esac
require_absolute WORKSPACE "$workspace"
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

[ -n "$stage_source" ] || fail "FN64_STAGE_SNAPSHOT_BANK is required"
require_absolute FN64_STAGE_SNAPSHOT_BANK "$stage_source"
[ -f "$stage_source" ] && [ -x "$stage_source" ] && [ ! -L "$stage_source" ] ||
    fail "FN64_STAGE_SNAPSHOT_BANK must be an executable regular non-symlink file"

jdk=${GHIDRA_JAVA_HOME:-}
[ -n "$jdk" ] || fail "GHIDRA_JAVA_HOME is required"
require_absolute GHIDRA_JAVA_HOME "$jdk"
[ -x "$jdk/bin/java" ] || fail "GHIDRA_JAVA_HOME does not contain bin/java"
[ -x "$jdk/bin/jar" ] || fail "GHIDRA_JAVA_HOME does not contain bin/jar"

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
[ -x "$launcher_verifier_source" ] || fail "Ghidra launcher verifier is not executable"
"$launcher_verifier_source" "$ghidra_install" "$headless" ||
    fail "GHIDRA_HEADLESS does not belong to GHIDRA_INSTALL_DIR"
application_properties="$ghidra_install/Ghidra/application.properties"
[ -f "$application_properties" ] || fail "Ghidra application.properties is missing"

for source_spec in \
        "memory guard:$guard_source" \
        "distribution manifest helper:$manifest_source" \
        "source policy:$policy_source" \
        "artifact policy:$artifact_policy_source" \
        "provenance verifier:$verifier_source" \
        "Ghidra launcher verifier:$launcher_verifier_source" \
        "N64LoaderWV install verifier:$install_verifier_source" \
        "N64LoaderWV runtime verifier:$runtime_verifier_source" \
        "comparison exporter:$exporter_source" \
        "comparison tool:$comparator_source"; do
    source_label=${source_spec%%:*}
    source_path=${source_spec#*:}
    [ -f "$source_path" ] && [ ! -L "$source_path" ] ||
        fail "$source_label must be a regular non-symlink file"
done
[ -x "$guard_source" ] || fail "memory guard is not executable"
[ -x "$manifest_source" ] || fail "distribution manifest helper is not executable"
[ -x "$verifier_source" ] || fail "provenance verifier is not executable"
[ -x "$install_verifier_source" ] || fail "N64LoaderWV install verifier is not executable"
[ -x "$comparator_source" ] || fail "comparison tool is not executable"
command -v python3 >/dev/null 2>&1 || fail "python3 is required"
command -v unzip >/dev/null 2>&1 || fail "unzip is required"

# Every N64LoaderWV invocation must start from the receipt-bound approved artifact.
"$verifier_source" artifact "$artifact_policy_source" "$policy_source" \
    "$conformance_receipt" "$extension_zip" \
    >/dev/null || fail "extension artifact does not satisfy the fn64 source policy"

if free_kib=$(df -Pk "$workspace" | awk 'NR == 2 {print $4}'); then
    case "$free_kib" in
        ''|*[!0-9]*) fail "could not measure workspace free disk" ;;
    esac
    [ "$free_kib" -ge 2097152 ] || fail "WORKSPACE has less than 2 GiB free disk"
else
    fail "could not measure workspace free disk"
fi

attempt=$(mktemp -d "$workspace/ghidra-snapshot-loader-ab.XXXXXXXX") ||
    fail "could not create private attempt"
chmod 700 "$attempt"
mkdir -m 700 "$attempt/inputs" "$attempt/raw" "$attempt/config" \
    "$attempt/tool-artifacts" "$attempt/diagnostics" "$attempt/out" \
    "$attempt/lanes" "$attempt/lanes/binary" "$attempt/lanes/n64"

bound_runner="$attempt/tool-artifacts/run-snapshot-loader-ab.sh"
bound_guard="$attempt/tool-artifacts/memory-guard.zsh"
bound_stage="$attempt/tool-artifacts/stage_snapshot_bank"
bound_manifest="$attempt/tool-artifacts/manifest-ghidra-distribution.py"
bound_policy="$attempt/tool-artifacts/n64loaderwv-source-policy.json"
bound_artifact_policy="$attempt/tool-artifacts/n64loaderwv-artifact-policy.json"
bound_verifier="$attempt/tool-artifacts/verify-n64loaderwv-provenance.py"
bound_launcher_verifier="$attempt/tool-artifacts/verify-ghidra-launcher.py"
bound_install_verifier="$attempt/tool-artifacts/verify-n64loaderwv-install.py"
bound_runtime_verifier="$attempt/tool-artifacts/Fn64VerifyN64LoaderRuntime.java"
bound_exporter="$attempt/tool-artifacts/Fn64ExportLoaderComparison.java"
bound_comparator="$attempt/tool-artifacts/compare-snapshot-loader-ab.py"
cp -- "$runner_source" "$bound_runner"
cp -- "$guard_source" "$bound_guard"
cp -- "$stage_source" "$bound_stage"
cp -- "$manifest_source" "$bound_manifest"
cp -- "$policy_source" "$bound_policy"
cp -- "$artifact_policy_source" "$bound_artifact_policy"
cp -- "$verifier_source" "$bound_verifier"
cp -- "$launcher_verifier_source" "$bound_launcher_verifier"
cp -- "$install_verifier_source" "$bound_install_verifier"
cp -- "$runtime_verifier_source" "$bound_runtime_verifier"
cp -- "$exporter_source" "$bound_exporter"
cp -- "$comparator_source" "$bound_comparator"
chmod 700 "$bound_guard" "$bound_stage" "$bound_manifest" "$bound_verifier" \
    "$bound_launcher_verifier" "$bound_install_verifier" "$bound_comparator"
chmod 600 "$bound_runner" "$bound_policy" "$bound_artifact_policy" \
    "$bound_runtime_verifier" "$bound_exporter"

runner_sha=$(hash_file "$bound_runner")
guard_sha=$(hash_file "$bound_guard")
stage_sha=$(hash_file "$bound_stage")
manifest_helper_sha=$(hash_file "$bound_manifest")
policy_sha=$(hash_file "$bound_policy")
artifact_policy_sha=$(hash_file "$bound_artifact_policy")
verifier_sha=$(hash_file "$bound_verifier")
launcher_verifier_sha=$(hash_file "$bound_launcher_verifier")
install_verifier_sha=$(hash_file "$bound_install_verifier")
runtime_verifier_sha=$(hash_file "$bound_runtime_verifier")
exporter_sha=$(hash_file "$bound_exporter")
comparator_sha=$(hash_file "$bound_comparator")

active_guard_pid=
launching_guard=0
interrupted_signal=
idle_phase=before_distribution_scan

write_interruption_receipt() {
    python3 - "$attempt/diagnostics/runner-interruption.json" "$1" "$2" "$3" <<'PY'
import json
import sys
path, signal_name, phase, guard_status = sys.argv[1:]
value = {
    "schema": "fn64.ghidra-snapshot-loader-ab-interruption",
    "schema_version": 1,
    "signal": signal_name,
    "phase": phase,
    "guard_exit_status": int(guard_status),
    "active_guard_cleanup_complete": True,
}
with open(path, "x", encoding="utf-8") as stream:
    stream.write(json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n")
PY
    chmod 600 "$attempt/diagnostics/runner-interruption.json"
}

finish_interruption() {
    trap '' HUP INT TERM
    write_interruption_receipt "$interrupted_signal" "$1" "$2"
    signal_name=$interrupted_signal
    trap - HUP INT TERM
    case "$signal_name" in
        HUP) exit 129 ;;
        INT) exit 130 ;;
        TERM) exit 143 ;;
        *) exit 128 ;;
    esac
}

forward_runner_signal() {
    signal_name=$1
    [ -n "$interrupted_signal" ] || interrupted_signal=$signal_name
    if [ -z "$active_guard_pid" ] && [ "$launching_guard" -eq 0 ]; then
        finish_interruption "$idle_phase" 0
    fi
}

exit_if_interrupted() {
    idle_phase=$1
    [ -z "$interrupted_signal" ] || finish_interruption "$idle_phase" 0
}

run_guarded_phase() {
    phase=$1
    guard_jsonl=$2
    poll_seconds=$3
    shift 3
    exit_if_interrupted "before_$phase"
    launching_guard=1
    FN64_GUARD_MAX_RSS_MIB=2048 \
    FN64_GUARD_MIN_FREE_PERCENT=40 \
    FN64_GUARD_MAX_SECONDS=180 \
    FN64_GUARD_POLL_SECONDS="$poll_seconds" \
    FN64_GUARD_JSONL="$guard_jsonl" \
        "$bound_guard" "$@" &
    active_guard_pid=$!
    launching_guard=0
    if [ -n "$interrupted_signal" ]; then
        while [ ! -s "$guard_jsonl" ] && kill -0 "$active_guard_pid" 2>/dev/null; do
            sleep 0.01
        done
        kill -TERM "$active_guard_pid" 2>/dev/null || true
    fi
    set +e
    wait "$active_guard_pid"
    phase_status=$?
    set -e
    active_guard_pid=
    idle_phase="after_$phase"
    [ -z "$interrupted_signal" ] || finish_interruption "$phase" "$phase_status"
    return "$phase_status"
}

trap 'forward_runner_signal HUP' HUP
trap 'forward_runner_signal INT' INT
trap 'forward_runner_signal TERM' TERM

path_value=${PATH:-/usr/bin:/bin}
distribution_cache="$workspace/.fn64-ghidra-distribution-manifests"
if [ -e "$distribution_cache" ]; then
    [ -d "$distribution_cache" ] && [ ! -L "$distribution_cache" ] ||
        fail "Ghidra distribution manifest cache must be a non-symlink directory"
else
    mkdir -m 700 "$distribution_cache"
fi
[ "$(file_mode "$distribution_cache")" = 700 ] ||
    fail "Ghidra distribution manifest cache must have mode 0700"

distribution_manifest="$attempt/tool-artifacts/ghidra-distribution.json"
distribution_scan_log="$attempt/diagnostics/ghidra-distribution-scan.log"
distribution_scan_guard="$attempt/diagnostics/ghidra-distribution-scan-memory.jsonl"
run_guarded_phase distribution_scan "$distribution_scan_guard" 0.1 \
    env -i "PATH=$path_value" "HOME=$attempt" "TMPDIR=$attempt" \
    "$bound_manifest" scan "$ghidra_install" "$distribution_cache" \
        "$distribution_manifest" >"$distribution_scan_log" 2>&1 ||
    fail "Ghidra distribution inventory failed; see $distribution_scan_log"
[ -s "$distribution_manifest" ] || fail "Ghidra distribution inventory is empty"
distribution_sha=$(hash_file "$distribution_manifest")

snapshot_copy="$attempt/inputs/program-snapshot.json"
bank_copy="$attempt/inputs/materialized-bank.bin"
extension_copy="$attempt/inputs/n64loaderwv-extension.zip"
receipt_copy="$attempt/inputs/n64loaderwv-conformance-receipt.txt"
cp -- "$program_snapshot" "$snapshot_copy"
cp -- "$materialized_bank" "$bank_copy"
cp -- "$extension_zip" "$extension_copy"
cp -- "$conformance_receipt" "$receipt_copy"
chmod 600 "$snapshot_copy" "$bank_copy" "$extension_copy" "$receipt_copy"
[ "$(hash_file "$program_snapshot")" = "$(hash_file "$snapshot_copy")" ] ||
    fail "PROGRAM_SNAPSHOT changed while copying"
[ "$(hash_file "$materialized_bank")" = "$(hash_file "$bank_copy")" ] ||
    fail "MATERIALIZED_BANK changed while copying"
[ "$(hash_file "$extension_zip")" = "$(hash_file "$extension_copy")" ] ||
    fail "EXTENSION_ZIP changed while copying"
[ "$(hash_file "$conformance_receipt")" = "$(hash_file "$receipt_copy")" ] ||
    fail "CONFORMANCE_RECEIPT changed while copying"
"$bound_launcher_verifier" "$ghidra_install" "$headless" ||
    fail "bound Ghidra launcher verification failed"
verified_artifact=$("$bound_verifier" artifact "$bound_artifact_policy" \
    "$bound_policy" "$receipt_copy" "$extension_copy") ||
    fail "copied extension artifact failed provenance replay"

verified_fields="$attempt/diagnostics/verified-loader-fields.txt"
printf '%s\n' "$verified_artifact" | python3 -c \
    'import json,sys; v=json.load(sys.stdin); print(v["repository"]); print(v["policy_sha256"]); print(v["commit"]); print(v["tree"]); print(v["source_archive_sha256"]); print(v["extension_sha256"]); print(v["conformance_receipt_sha256"])' \
    > "$verified_fields" || fail "provenance verifier returned invalid JSON"
loader_repository=$(sed -n '1p' "$verified_fields")
loader_policy_sha=$(sed -n '2p' "$verified_fields")
loader_commit=$(sed -n '3p' "$verified_fields")
loader_tree=$(sed -n '4p' "$verified_fields")
source_archive_sha=$(sed -n '5p' "$verified_fields")
extension_sha=$(sed -n '6p' "$verified_fields")
conformance_receipt_sha=$(sed -n '7p' "$verified_fields")

staged_bank="$attempt/inputs/bank.bin"
evidence="$attempt/raw/evidence.json"
stage_log="$attempt/diagnostics/stage.log"
stage_guard="$attempt/diagnostics/stage-memory.jsonl"
run_guarded_phase stage "$stage_guard" 0.1 \
    env -i "PATH=$path_value" "HOME=$attempt" "TMPDIR=$attempt" \
    "$bound_stage" --discovery-only "$snapshot_copy" "$bank" "$bank_copy" \
        "$workspace" "$staged_bank" "$evidence" >"$stage_log" 2>&1 ||
    fail "snapshot-bank staging failed; see $stage_log"
[ -s "$staged_bank" ] && [ -s "$evidence" ] || fail "snapshot-bank staging produced empty output"

stage_fields="$attempt/diagnostics/stage-fields.txt"
python3 - "$evidence" "$bank" <<'PY' > "$stage_fields" ||
import json
import re
import sys
path, expected_bank = sys.argv[1:]
with open(path, "r", encoding="utf-8") as stream:
    value = json.load(stream)
if set(value) != {"schema", "schema_version", "program_snapshot_sha256", "input", "backing", "artifact", "seeds"}:
    raise SystemExit("wrong evidence fields")
if value["schema"] != "fn64.snapshot-bank-evidence" or value["schema_version"] != 3:
    raise SystemExit("wrong discovery-only evidence schema")
if value["seeds"] != {"mode": "discovery_only", "role": "candidate_only"}:
    raise SystemExit("snapshot staging was not discovery-only")
input_value = value["input"]
if input_value.get("bank") != expected_bank:
    raise SystemExit("wrong evidence bank")
for field in ("program_snapshot_sha256",):
    if not isinstance(value[field], str) or not re.fullmatch(r"[0-9a-f]{64}", value[field]):
        raise SystemExit("invalid snapshot digest")
for field in ("normalized_rom_sha256", "bank_bytes_sha256", "mapping_sha256"):
    item = input_value.get(field)
    if not isinstance(item, str) or not re.fullmatch(r"[0-9a-f]{64}", item):
        raise SystemExit(f"invalid {field}")
for field in ("va_start", "va_end"):
    item = input_value.get(field)
    if not isinstance(item, int) or isinstance(item, bool) or item < 0:
        raise SystemExit(f"invalid {field}")
artifact = value["artifact"]
if artifact.get("sha256") != input_value["bank_bytes_sha256"]:
    raise SystemExit("artifact and bank digests differ")
print(value["program_snapshot_sha256"])
print(input_value["normalized_rom_sha256"])
print(input_value["bank_bytes_sha256"])
print(input_value["mapping_sha256"])
print(input_value["va_start"])
print(input_value["va_end"])
print(artifact.get("byte_length"))
PY
    fail "stage_snapshot_bank produced invalid evidence"
snapshot_sha=$(sed -n '1p' "$stage_fields")
rom_sha=$(sed -n '2p' "$stage_fields")
bank_sha=$(sed -n '3p' "$stage_fields")
mapping_sha=$(sed -n '4p' "$stage_fields")
va_start=$(sed -n '5p' "$stage_fields")
va_end=$(sed -n '6p' "$stage_fields")
bank_length=$(sed -n '7p' "$stage_fields")
[ "$bank_sha" = "$(hash_file "$staged_bank")" ] || fail "staged bank digest is wrong"
[ "$bank_length" -eq "$(wc -c < "$staged_bank" | tr -d ' ')" ] ||
    fail "staged bank length is wrong"

context_start=2147483648
context_4m_end=2151677952
context_8m_end=2155872256
[ "$va_start" -ge "$context_start" ] && [ "$va_start" -lt "$va_end" ] ||
    fail "bank interval is outside KSEG0 RDRAM"
[ $((va_start % 4)) -eq 0 ] && [ $((va_end % 4)) -eq 0 ] ||
    fail "bank interval is not word-aligned"
if [ "$va_end" -le "$context_4m_end" ]; then
    context_end=$context_4m_end
elif [ "$va_end" -le "$context_8m_end" ]; then
    context_end=$context_8m_end
else
    fail "bank interval does not fit supported 4/8 MiB RDRAM"
fi
context_length=$((context_end - context_start))
bank_offset=$((va_start - context_start))

rdram="$attempt/inputs/rdram.bin"
synthetic_rom="$attempt/inputs/synthetic.z64"
python3 - "$staged_bank" "$rdram" "$synthetic_rom" "$context_length" "$bank_offset" "$va_start" <<'PY' ||
import pathlib
import sys
bank_path, rdram_path, rom_path, context_length, bank_offset, entry = sys.argv[1:]
bank = pathlib.Path(bank_path).read_bytes()
length = int(context_length)
offset = int(bank_offset)
if offset < 0 or offset + len(bank) > length:
    raise SystemExit("bank does not fit synthesized context")
rdram = bytearray(length)
rdram[offset:offset + len(bank)] = bank
with open(rdram_path, "xb") as stream:
    stream.write(rdram)
rom = bytearray(8192)
rom[0:4] = bytes.fromhex("80371240")
rom[8:12] = int(entry).to_bytes(4, "big")
with open(rom_path, "xb") as stream:
    stream.write(rom)
PY
    fail "could not synthesize shared RDRAM context"
chmod 600 "$rdram" "$synthetic_rom"
[ "$(wc -c < "$rdram" | tr -d ' ')" -eq "$context_length" ] ||
    fail "synthesized RDRAM has the wrong size"
context_sha=$(hash_file "$rdram")
synthetic_rom_sha=$(hash_file "$synthetic_rom")
python3 - "$rdram" "$bank_offset" "$bank_length" "$bank_sha" <<'PY' ||
import hashlib
import pathlib
import sys
path, offset, length, expected = sys.argv[1:]
with open(path, "rb") as stream:
    stream.seek(int(offset))
    data = stream.read(int(length))
if len(data) != int(length) or hashlib.sha256(data).hexdigest() != expected:
    raise SystemExit("embedded bank digest mismatch")
PY
    fail "synthesized RDRAM does not contain the exact staged bank"

ghidra_version=$(awk -F= '$1 == "application.version" { print $2 }' "$application_properties")
ghidra_release=$(awk -F= '$1 == "application.release.name" { print $2 }' "$application_properties")
[ -n "$ghidra_version" ] && [ -n "$ghidra_release" ] || fail "could not read Ghidra identity"
java_sha=$(hash_file "$jdk/bin/java")
common_provenance_sha=$(hash_fields fn64.ghidra-loader-ab.provenance.v1 \
    "$snapshot_sha" "$rom_sha" "$bank" "$bank_sha" "$mapping_sha" \
    "$va_start" "$va_end" "$context_start" "$context_end" "$context_sha" \
    "$synthetic_rom_sha" "$distribution_sha" "$java_sha" "$exporter_sha" \
    "$artifact_policy_sha" "$launcher_verifier_sha" "$install_verifier_sha" \
    "$runtime_verifier_sha")

make_config() {
    lane=$1
    loader=$2
    output=$3
    python3 - "$output" "$lane" "$loader" "$ghidra_version" "$snapshot_sha" \
        "$rom_sha" "$bank" "$bank_sha" "$mapping_sha" "$va_start" "$va_end" \
        "$context_start" "$context_end" "$context_sha" "$synthetic_rom_sha" \
        "$distribution_sha" "$java_sha" "$exporter_sha" "$common_provenance_sha" \
        "$loader_repository" "$loader_policy_sha" "$loader_commit" "$loader_tree" \
        "$source_archive_sha" "$extension_sha" "$conformance_receipt_sha" <<'PY'
import json
import sys
(path, lane, loader, ghidra_version, snapshot_sha, rom_sha, bank, bank_sha, mapping_sha,
 va_start, va_end, context_start, context_end, context_sha, synthetic_rom_sha,
 distribution_sha, java_sha, exporter_sha, provenance_sha, loader_repository,
 loader_policy_sha, loader_commit, loader_tree, source_archive_sha, extension_sha,
 conformance_receipt_sha) = sys.argv[1:]
value = {
    "schema": "fn64.ghidra-loader-ab-config",
    "schema_version": 1,
    "lane": lane,
    "loader": loader,
    "processor": "MIPS:BE:64:64-32addr:o32",
    "seed_policy": "loader_native_only",
    "input": {
        "program_snapshot_sha256": snapshot_sha,
        "normalized_rom_sha256": rom_sha,
        "bank": bank,
        "bank_bytes_sha256": bank_sha,
        "mapping_sha256": mapping_sha,
        "va_start": int(va_start),
        "va_end": int(va_end),
        "context_start": int(context_start),
        "context_end": int(context_end),
        "context_sha256": context_sha,
        "synthetic_container_sha256": synthetic_rom_sha,
    },
    "tool": {
        "ghidra_version": ghidra_version,
        "ghidra_distribution_sha256": distribution_sha,
        "java_sha256": java_sha,
        "exporter_sha256": exporter_sha,
        "provenance_sha256": provenance_sha,
    },
    "resources": {
        "analysis_timeout_seconds": 120,
        "max_cpu": 1,
        "heap_mib": 1024,
        "rss_mib": 2048,
        "min_free_percent": 40,
        "wall_seconds": 180,
    },
}
if lane == "n64loaderwv":
    value["n64loaderwv"] = {
        "repository": loader_repository,
        "policy_sha256": loader_policy_sha,
        "commit": loader_commit,
        "tree": loader_tree,
        "source_archive_sha256": source_archive_sha,
        "extension_sha256": extension_sha,
        "conformance_receipt_sha256": conformance_receipt_sha,
    }
with open(path, "x", encoding="utf-8") as stream:
    stream.write(json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n")
PY
}

binary_config="$attempt/config/binary.json"
n64_config="$attempt/config/n64.json"
make_config binary-loader BinaryLoader "$binary_config"
make_config n64loaderwv N64LoaderWVLoader "$n64_config"
binary_provenance_sha=$(hash_fields "$common_provenance_sha" "$(hash_file "$binary_config")")
n64_provenance_sha=$(hash_fields "$common_provenance_sha" "$(hash_file "$n64_config")" \
    "$extension_sha" "$conformance_receipt_sha")

archive_entries="$attempt/diagnostics/extension-archive-entries.txt"
unzip -Z1 "$extension_copy" > "$archive_entries"
if grep -Eq '(^/|(^|/)\.\.(/|$))' "$archive_entries"; then
    fail "extension archive contains an unsafe path"
fi
extension_roots=$(awk -F/ 'NF { print $1 }' "$archive_entries" | sort -u)
[ "$(printf '%s\n' "$extension_roots" | awk 'NF { count++ } END { print count + 0 }')" -eq 1 ] ||
    fail "extension archive must have exactly one top-level directory"
extension_root=$(printf '%s\n' "$extension_roots" | awk 'NF { print; exit }')
case "$extension_root" in
    ''|.|..|*[!A-Za-z0-9._-]*) fail "extension archive has an invalid top-level directory" ;;
esac

run_lane() {
    lane=$1
    lane_label=$2
    lane_root="$attempt/lanes/$lane"
    mkdir -m 700 "$lane_root/project" "$lane_root/home" "$lane_root/tmp" \
        "$lane_root/cache" "$lane_root/out" "$lane_root/diagnostics"
    pre="$lane_root/out/pre.json"
    post="$lane_root/out/post.json"
    analysis_log="$lane_root/diagnostics/analyze.log"
    analysis_guard="$lane_root/diagnostics/memory.jsonl"
    if [ "$lane" = n64 ]; then
        settings_user="$lane_root/home/ghidra/ghidra_${ghidra_version}_${ghidra_release}"
        extensions="$settings_user/Extensions"
        mkdir -p "$extensions"
        unzip -q "$extension_copy" -d "$extensions"
        [ -f "$extensions/$extension_root/extension.properties" ] ||
            fail "installed extension is missing extension.properties"
        [ -f "$extensions/$extension_root/lib/$extension_root.jar" ] ||
            fail "installed extension is missing its module JAR"
        n64_install_receipt="$lane_root/out/install-verification.json"
        "$bound_install_verifier" "$extension_copy" "$extensions/$extension_root" \
            "$ghidra_install" "$settings_user" > "$n64_install_receipt" ||
            fail "installed extension does not exactly match the approved fn64 fork"
        loader_jar_sha=$(python3 -c \
            'import json,sys; print(json.load(open(sys.argv[1], encoding="utf-8"))["loader_jar"]["sha256"])' \
            "$n64_install_receipt")
        loader_class_sha=$(python3 -c \
            'import json,sys; print(json.load(open(sys.argv[1], encoding="utf-8"))["loader_class"]["sha256"])' \
            "$n64_install_receipt")
        loader_jar="$extensions/$extension_root/lib/$extension_root.jar"
        n64_runtime_receipt="$lane_root/out/runtime-verification.json"
        import_file=$synthetic_rom
        program_name=$(basename -- "$synthetic_rom")
        provenance_sha=$n64_provenance_sha
        run_guarded_phase analysis_n64 "$analysis_guard" 1 \
            env -i "PATH=$path_value" "HOME=$lane_root/home" "TMPDIR=$lane_root/tmp" \
            "JAVA_HOME=$jdk" "GHIDRA_HEADLESS_MAXMEM=1G" \
            "_JAVA_OPTIONS=-Dapplication.settingsdir=$lane_root/home -Dapplication.cachedir=$lane_root/cache -Dapplication.tempdir=$lane_root/tmp -Djava.io.tmpdir=$lane_root/tmp -Duser.home=$lane_root/home" \
            "$headless" "$lane_root/project" snapshot-loader-ab-n64 \
                -import "$import_file" -overwrite \
                -loader N64LoaderWVLoader -loader-rdram "$rdram" \
                -scriptPath "$attempt/tool-artifacts" \
                -preScript Fn64ExportLoaderComparison.java "$pre" "$lane_label" pre "$bank" \
                    "$va_start" "$va_end" "$context_start" "$context_end" "$rom_sha" \
                    "$bank_sha" "$context_sha" "$mapping_sha" "$provenance_sha" "$program_name" \
                -analysisTimeoutPerFile 120 -max-cpu 1 \
                -postScript Fn64ExportLoaderComparison.java "$post" "$lane_label" post "$bank" \
                    "$va_start" "$va_end" "$context_start" "$context_end" "$rom_sha" \
                    "$bank_sha" "$context_sha" "$mapping_sha" "$provenance_sha" "$program_name" \
                -postScript Fn64VerifyN64LoaderRuntime.java \
                    "$n64_runtime_receipt" "$loader_jar" "$loader_jar_sha" "$loader_class_sha" \
                    'N64 Loader by Warranty Voider' 'N64 Loader by Warranty Voider' \
                -deleteProject >"$analysis_log" 2>&1 ||
            fail "N64LoaderWV analysis failed; see $analysis_log"
        [ -s "$n64_runtime_receipt" ] ||
            fail "N64 lane did not verify the loaded N64LoaderWV runtime"
        if grep -q "Ignoring class 'n64loaderwv.N64LoaderWVLoader'" "$analysis_log"; then
            fail "another N64LoaderWV installation shadowed the isolated extension"
        fi
        grep -q 'Using Loader: N64 Loader by Warranty Voider' "$analysis_log" ||
            fail "N64 lane did not select N64LoaderWV"
        if grep -q 'Using Loader: Raw Binary' "$analysis_log"; then
            fail "N64 lane unexpectedly selected BinaryLoader"
        fi
    else
        import_file=$rdram
        program_name=$(basename -- "$rdram")
        provenance_sha=$binary_provenance_sha
        run_guarded_phase analysis_binary "$analysis_guard" 1 \
            env -i "PATH=$path_value" "HOME=$lane_root/home" "TMPDIR=$lane_root/tmp" \
            "JAVA_HOME=$jdk" "GHIDRA_HEADLESS_MAXMEM=1G" \
            "_JAVA_OPTIONS=-Dapplication.settingsdir=$lane_root/home -Dapplication.cachedir=$lane_root/cache -Dapplication.tempdir=$lane_root/tmp -Djava.io.tmpdir=$lane_root/tmp -Duser.home=$lane_root/home" \
            "$headless" "$lane_root/project" snapshot-loader-ab-binary \
                -import "$import_file" -overwrite \
                -processor MIPS:BE:64:64-32addr -cspec o32 \
                -loader BinaryLoader -loader-baseAddr 80000000 \
                -scriptPath "$attempt/tool-artifacts" \
                -preScript Fn64ExportLoaderComparison.java "$pre" "$lane_label" pre "$bank" \
                    "$va_start" "$va_end" "$context_start" "$context_end" "$rom_sha" \
                    "$bank_sha" "$context_sha" "$mapping_sha" "$provenance_sha" "$program_name" \
                -analysisTimeoutPerFile 120 -max-cpu 1 \
                -postScript Fn64ExportLoaderComparison.java "$post" "$lane_label" post "$bank" \
                    "$va_start" "$va_end" "$context_start" "$context_end" "$rom_sha" \
                    "$bank_sha" "$context_sha" "$mapping_sha" "$provenance_sha" "$program_name" \
                -deleteProject >"$analysis_log" 2>&1 ||
            fail "BinaryLoader analysis failed; see $analysis_log"
        grep -q 'Using Loader: Raw Binary' "$analysis_log" ||
            fail "Binary lane did not select BinaryLoader"
        grep -q 'Using Language/Compiler: MIPS:BE:64:64-32addr:o32' "$analysis_log" ||
            fail "Binary lane did not select pinned MIPS/o32"
        if grep -q 'Using Loader: N64 Loader by Warranty Voider' "$analysis_log"; then
            fail "Binary lane unexpectedly selected N64LoaderWV"
        fi
    fi
    [ -s "$pre" ] && [ -s "$post" ] || fail "$lane_label exporter output is incomplete"
    if find "$lane_root/project" -mindepth 1 -print -quit | grep -q .; then
        fail "$lane_label Ghidra project was not deleted"
    fi
    [ "$(wc -c < "$analysis_log" | tr -d ' ')" -le 16777216 ] ||
        fail "$lane_label analysis log exceeds 16 MiB"
}

run_lane binary binary-loader
run_lane n64 n64loaderwv

validate_inventory() {
    path=$1
    phase=$2
    lane=$3
    expected_provenance=$4
    python3 - "$path" "$phase" "$lane" "$bank" "$rom_sha" "$context_sha" "$bank_sha" \
        "$mapping_sha" "$va_start" "$va_end" "$context_start" "$context_end" \
        "$expected_provenance" <<'PY'
import json
import sys
(path, phase, lane, bank, rom_sha, context_sha, bank_sha, mapping_sha,
 va_start, va_end, context_start, context_end, provenance_sha) = sys.argv[1:]
with open(path, "r", encoding="utf-8") as stream:
    value = json.load(stream)
if value.get("schema") != "fn64.ghidra-bank-function-inventory" or value.get("schema_version") != 4:
    raise SystemExit("wrong inventory schema")
if value.get("candidate_only") is not True:
    raise SystemExit("wrong inventory phase/role")
if value.get("provenance") != {
    "lane": lane, "phase": phase, "source_sha256": provenance_sha
}:
    raise SystemExit("wrong inventory provenance")
input_value = value.get("input", {})
expected = {
    "normalized_rom_sha256": rom_sha,
    "bank": bank,
    "bank_bytes_sha256": bank_sha,
    "mapping_sha256": mapping_sha,
    "va_start": int(va_start),
    "va_end": int(va_end),
    "context_bytes_sha256": context_sha,
    "context_start": int(context_start),
    "context_end": int(context_end),
}
if input_value != expected:
    raise SystemExit("wrong inventory input")
if (not isinstance(value.get("memory_blocks"), list) or
        not isinstance(value.get("entry_points"), list) or
        not isinstance(value.get("rejected_functions"), list) or
        not isinstance(value.get("functions"), list)):
    raise SystemExit("inventory arrays are missing")
PY
}

validate_inventory "$attempt/lanes/binary/out/pre.json" pre binary-loader "$binary_provenance_sha" ||
    fail "invalid BinaryLoader pre-analysis inventory"
validate_inventory "$attempt/lanes/binary/out/post.json" post binary-loader "$binary_provenance_sha" ||
    fail "invalid BinaryLoader post-analysis inventory"
validate_inventory "$attempt/lanes/n64/out/pre.json" pre n64loaderwv "$n64_provenance_sha" ||
    fail "invalid N64LoaderWV pre-analysis inventory"
validate_inventory "$attempt/lanes/n64/out/post.json" post n64loaderwv "$n64_provenance_sha" ||
    fail "invalid N64LoaderWV post-analysis inventory"

comparison="$attempt/out/comparison.json"
comparison_guard="$attempt/diagnostics/comparison-memory.jsonl"
comparison_log="$attempt/diagnostics/comparison.log"
run_guarded_phase comparison "$comparison_guard" 0.1 \
    env -i "PATH=$path_value" "HOME=$attempt" "TMPDIR=$attempt" \
    "$bound_comparator" "$attempt/lanes/binary/out/pre.json" \
        "$attempt/lanes/binary/out/post.json" "$attempt/lanes/n64/out/pre.json" \
        "$attempt/lanes/n64/out/post.json" "$comparison" >"$comparison_log" 2>&1 ||
    fail "loader comparison failed; see $comparison_log"
[ -s "$comparison" ] || fail "loader comparison produced no report"
python3 - "$comparison" <<'PY' || fail "loader comparison produced an invalid report"
import json
import sys
with open(sys.argv[1], "r", encoding="utf-8") as stream:
    value = json.load(stream)
if value.get("schema") != "fn64.ghidra-loader-ab" or value.get("schema_version") != 1:
    raise SystemExit("wrong comparison schema")
if value.get("authority") != "candidate_only":
    raise SystemExit("comparison is not candidate-only")
if value.get("role") != "differential_comparison" or value.get("context") != "shared_mapped_bytes":
    raise SystemExit("comparison has the wrong diagnostic role/context")
PY

distribution_verify_log="$attempt/diagnostics/ghidra-distribution-verify.log"
distribution_verify_guard="$attempt/diagnostics/ghidra-distribution-verify-memory.jsonl"
run_guarded_phase distribution_verify "$distribution_verify_guard" 0.1 \
    env -i "PATH=$path_value" "HOME=$attempt" "TMPDIR=$attempt" \
    "$bound_manifest" verify "$ghidra_install" "$distribution_manifest" \
        >"$distribution_verify_log" 2>&1 ||
    fail "Ghidra distribution changed during A/B analysis"
[ "$(hash_file "$distribution_manifest")" = "$distribution_sha" ] ||
    fail "Ghidra distribution manifest changed during A/B analysis"

for source_pair in \
        "$runner_source:$runner_sha" \
        "$guard_source:$guard_sha" \
        "$stage_source:$stage_sha" \
        "$manifest_source:$manifest_helper_sha" \
        "$policy_source:$policy_sha" \
        "$artifact_policy_source:$artifact_policy_sha" \
        "$verifier_source:$verifier_sha" \
        "$launcher_verifier_source:$launcher_verifier_sha" \
        "$install_verifier_source:$install_verifier_sha" \
        "$runtime_verifier_source:$runtime_verifier_sha" \
        "$exporter_source:$exporter_sha" \
        "$comparator_source:$comparator_sha"; do
    source_path=${source_pair%:*}
    expected_sha=${source_pair##*:}
    [ "$(hash_file "$source_path")" = "$expected_sha" ] ||
        fail "bound source changed during A/B analysis: $source_path"
done

attempt_kib=$(du -sk "$attempt" | awk '{print $1}')
case "$attempt_kib" in
    ''|*[!0-9]*) fail "could not measure attempt size" ;;
esac
[ "$attempt_kib" -le 524288 ] || fail "A/B attempt exceeds 512 MiB"

receipt="$attempt/out/receipt.json"
python3 - "$receipt" "$snapshot_sha" "$rom_sha" "$bank" "$bank_sha" "$mapping_sha" \
    "$va_start" "$va_end" "$context_start" "$context_end" "$context_sha" \
    "$synthetic_rom_sha" "$common_provenance_sha" "$loader_repository" \
    "$loader_policy_sha" "$loader_commit" "$loader_tree" "$source_archive_sha" \
    "$extension_sha" "$conformance_receipt_sha" "$distribution_sha" "$java_sha" \
    "$runner_sha" "$guard_sha" "$stage_sha" "$manifest_helper_sha" "$verifier_sha" \
    "$launcher_verifier_sha" "$install_verifier_sha" "$runtime_verifier_sha" \
    "$artifact_policy_sha" "$exporter_sha" \
    "$comparator_sha" "$(hash_file "$evidence")" \
    "$(hash_file "$binary_config")" "$(hash_file "$n64_config")" \
    "$(hash_file "$attempt/lanes/binary/out/pre.json")" \
    "$(hash_file "$attempt/lanes/binary/out/post.json")" \
    "$(hash_file "$attempt/lanes/n64/out/pre.json")" \
    "$(hash_file "$attempt/lanes/n64/out/post.json")" \
    "$(hash_file "$n64_install_receipt")" "$(hash_file "$n64_runtime_receipt")" \
    "$(hash_file "$comparison")" \
    "$(hash_file "$distribution_scan_guard")" \
    "$(hash_file "$attempt/lanes/binary/diagnostics/memory.jsonl")" \
    "$(hash_file "$attempt/lanes/n64/diagnostics/memory.jsonl")" \
    "$(hash_file "$comparison_guard")" "$(hash_file "$distribution_verify_guard")" <<'PY'
import json
import sys
keys = [
    "snapshot_sha", "rom_sha", "bank", "bank_sha", "mapping_sha", "va_start", "va_end",
    "context_start", "context_end", "context_sha", "synthetic_rom_sha", "provenance_sha",
    "loader_repository", "loader_policy_sha", "loader_commit", "loader_tree",
    "source_archive_sha", "extension_sha", "conformance_receipt_sha", "distribution_sha",
    "java_sha", "runner_sha", "guard_sha", "stage_sha", "manifest_helper_sha", "verifier_sha",
    "launcher_verifier_sha", "install_verifier_sha", "runtime_verifier_sha",
    "artifact_policy_sha", "exporter_sha", "comparator_sha",
    "evidence_sha", "binary_config_sha", "n64_config_sha",
    "binary_pre_sha", "binary_post_sha", "n64_pre_sha", "n64_post_sha",
    "install_verification_sha", "runtime_verification_sha", "comparison_sha",
    "distribution_scan_guard_sha", "binary_guard_sha", "n64_guard_sha", "comparison_guard_sha",
    "distribution_verify_guard_sha",
]
path = sys.argv[1]
values = sys.argv[2:]
if len(values) != len(keys):
    raise SystemExit("wrong receipt argument count")
v = dict(zip(keys, values))
value = {
    "schema": "fn64.ghidra-snapshot-loader-ab-receipt",
    "schema_version": 1,
    "candidate_only": True,
    "role": "differential_comparison",
    "context": "synthetic_zero_fill",
    "program_snapshot_sha256": v["snapshot_sha"],
    "input": {
        "normalized_rom_sha256": v["rom_sha"], "bank": v["bank"],
        "bank_bytes_sha256": v["bank_sha"], "mapping_sha256": v["mapping_sha"],
        "va_start": int(v["va_start"]), "va_end": int(v["va_end"]),
        "context_start": int(v["context_start"]), "context_end": int(v["context_end"]),
        "context_sha256": v["context_sha"],
        "synthetic_container_sha256": v["synthetic_rom_sha"],
    },
    "n64loaderwv": {
        "repository": v["loader_repository"], "policy_sha256": v["loader_policy_sha"],
        "commit": v["loader_commit"], "tree": v["loader_tree"],
        "source_archive_sha256": v["source_archive_sha"],
        "extension_sha256": v["extension_sha"],
        "conformance_receipt_sha256": v["conformance_receipt_sha"],
    },
    "tool_identity_sha256": {
        "common_provenance": v["provenance_sha"], "ghidra_distribution": v["distribution_sha"],
        "java": v["java_sha"], "runner": v["runner_sha"], "memory_guard": v["guard_sha"],
        "stage": v["stage_sha"], "distribution_manifest_helper": v["manifest_helper_sha"],
        "provenance_verifier": v["verifier_sha"],
        "ghidra_launcher_verifier": v["launcher_verifier_sha"],
        "n64loaderwv_install_verifier": v["install_verifier_sha"],
        "n64loaderwv_runtime_verifier": v["runtime_verifier_sha"],
        "n64loaderwv_artifact_policy": v["artifact_policy_sha"],
        "exporter": v["exporter_sha"],
        "comparator": v["comparator_sha"],
    },
    "artifact_sha256": {
        "evidence": v["evidence_sha"], "binary_config": v["binary_config_sha"],
        "n64_config": v["n64_config_sha"], "binary_pre": v["binary_pre_sha"],
        "binary_post": v["binary_post_sha"], "n64_pre": v["n64_pre_sha"],
        "n64_post": v["n64_post_sha"],
        "n64loaderwv_install_verification": v["install_verification_sha"],
        "n64loaderwv_runtime_verification": v["runtime_verification_sha"],
        "comparison": v["comparison_sha"],
    },
    "resource_evidence_sha256": {
        "distribution_scan": v["distribution_scan_guard_sha"],
        "binary": v["binary_guard_sha"], "n64": v["n64_guard_sha"],
        "comparison": v["comparison_guard_sha"],
        "distribution_verify": v["distribution_verify_guard_sha"],
    },
    "completed_lanes": ["binary-loader", "n64loaderwv"],
    "production_ingest_performed": False,
}
with open(path, "x", encoding="utf-8") as stream:
    stream.write(json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n")
PY
chmod 600 "$attempt"/inputs/* "$attempt"/raw/* "$attempt"/config/* \
    "$attempt"/diagnostics/* "$attempt"/out/* "$attempt"/lanes/binary/out/* \
    "$attempt"/lanes/binary/diagnostics/* "$attempt"/lanes/n64/out/* \
    "$attempt"/lanes/n64/diagnostics/* "$attempt"/tool-artifacts/*

echo "ghidra snapshot-loader-ab: complete"
echo "attempt=$attempt"
echo "comparison=$comparison"
echo "receipt=$receipt"
