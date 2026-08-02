#!/bin/sh
set -eu
umask 077

repo=$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd)
guard="$repo/scripts/memory-guard.zsh"
exporter="$repo/tools/ghidra/Fn64ExportReviewInventory.java"
policy="$repo/tools/ghidra/n64loaderwv-source-policy.json"
artifact_policy="$repo/tools/ghidra/n64loaderwv-artifact-policy.json"
provenance_verifier="$repo/tools/ghidra/verify-n64loaderwv-provenance.py"
launcher_verifier="$repo/tools/ghidra/verify-ghidra-launcher.py"
install_verifier="$repo/tools/ghidra/verify-n64loaderwv-install.py"
runtime_verifier="$repo/tools/ghidra/Fn64VerifyN64LoaderRuntime.java"

fail() {
    echo "n64loaderwv first-contact: $1" >&2
    exit 2
}

usage() {
    echo "usage: tools/ghidra/run-n64loaderwv-first-contact.sh ROM WORKSPACE EXTENSION_ZIP CONFORMANCE_RECEIPT [RDRAM]" >&2
    echo "requires: FN64_ROM_IDENTITY, GHIDRA_JAVA_HOME, and GHIDRA_INSTALL_DIR or GHIDRA_HEADLESS" >&2
    exit 2
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

require_absolute() {
    case "$2" in
        /*) ;;
        *) fail "$1 must be absolute" ;;
    esac
}

[ "$#" -eq 4 ] || [ "$#" -eq 5 ] || usage
rom=$1
workspace=$2
extension_zip=$3
build_receipt=$4
rdram=${5:-}

require_absolute ROM "$rom"
require_absolute WORKSPACE "$workspace"
require_absolute EXTENSION_ZIP "$extension_zip"
require_absolute CONFORMANCE_RECEIPT "$build_receipt"
[ -n "$rdram" ] && require_absolute RDRAM "$rdram"
[ -f "$rom" ] || fail "ROM is not a regular file"
[ ! -L "$rom" ] || fail "ROM must not be a symlink"
[ -d "$workspace" ] || fail "WORKSPACE must already exist"
[ ! -L "$workspace" ] || fail "WORKSPACE must not be a symlink"
[ -f "$extension_zip" ] || fail "EXTENSION_ZIP is not a regular file"
[ ! -L "$extension_zip" ] || fail "EXTENSION_ZIP must not be a symlink"
[ -f "$build_receipt" ] || fail "CONFORMANCE_RECEIPT is not a regular file"
[ ! -L "$build_receipt" ] || fail "CONFORMANCE_RECEIPT must not be a symlink"
if [ -n "$rdram" ]; then
    [ -f "$rdram" ] || fail "RDRAM is not a regular file"
    [ ! -L "$rdram" ] || fail "RDRAM must not be a symlink"
fi

[ -x "$provenance_verifier" ] || fail "N64LoaderWV provenance verifier is not executable"
[ -f "$policy" ] || fail "N64LoaderWV source policy is missing"
[ -f "$artifact_policy" ] || fail "N64LoaderWV artifact policy is missing"
command -v python3 >/dev/null 2>&1 || fail "python3 is required to verify provenance"
verified_artifact=$(
    "$provenance_verifier" artifact "$artifact_policy" "$policy" \
        "$build_receipt" "$extension_zip"
) || fail "extension artifact does not satisfy the fn64 source policy"
verified_field() {
    printf '%s\n' "$verified_artifact" | python3 -c \
        'import json,sys; value=json.load(sys.stdin); print(value[sys.argv[1]])' "$1"
}
loader_repository=$(verified_field repository)
loader_policy_sha=$(verified_field policy_sha256)
loader_commit=$(verified_field commit)
loader_tree=$(verified_field tree)
source_archive_sha=$(verified_field source_archive_sha256)
extension_sha=$(verified_field extension_sha256)
build_receipt_sha=$(verified_field conformance_receipt_sha256)

workspace=$(CDPATH='' cd -- "$workspace" && pwd -P) || fail "cannot resolve WORKSPACE"
case "$workspace" in
    "$repo"|"$repo"/*) fail "WORKSPACE must be outside the repository" ;;
esac
if workspace_mode=$(stat -c '%a' "$workspace" 2>/dev/null); then
    :
elif workspace_mode=$(stat -f '%Lp' "$workspace" 2>/dev/null); then
    :
else
    fail "could not inspect WORKSPACE permissions"
fi
[ "$workspace_mode" = 700 ] || fail "WORKSPACE must have mode 0700"

rom_identity=${FN64_ROM_IDENTITY:-}
[ -n "$rom_identity" ] || fail "FN64_ROM_IDENTITY is required; refusing to use a raw-file digest"
require_absolute FN64_ROM_IDENTITY "$rom_identity"
[ -x "$rom_identity" ] || fail "FN64_ROM_IDENTITY is not executable"

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
[ -x "$launcher_verifier" ] || fail "Ghidra launcher verifier is not executable"
"$launcher_verifier" "$ghidra_install" "$headless" ||
    fail "GHIDRA_HEADLESS does not belong to GHIDRA_INSTALL_DIR"
[ -f "$ghidra_install/Ghidra/application.properties" ] ||
    fail "Ghidra application.properties is missing"
[ -x "$jdk/bin/jar" ] || fail "GHIDRA_JAVA_HOME does not contain bin/jar"
[ -x "$guard" ] || fail "repository memory guard is not executable"
[ -f "$exporter" ] || fail "Fn64ExportReviewInventory.java is missing"
[ -x "$install_verifier" ] || fail "N64LoaderWV install verifier is not executable"
[ -f "$runtime_verifier" ] || fail "Fn64VerifyN64LoaderRuntime.java is missing"

header=$(od -An -tx1 -N4 "$rom" 2>/dev/null | tr -d ' \n')
case "$header" in
    80371240) expected_byte_order=z64 ;;
    40123780) expected_byte_order=n64 ;;
    37804012) expected_byte_order=v64 ;;
    *) fail "ROM header is not z64, n64, or v64" ;;
esac

identity_json=$("$rom_identity" "$rom") || fail "FN64_ROM_IDENTITY rejected the ROM"
identity_schema=$(printf '%s\n' "$identity_json" | sed -n 's/.*"schema":"\([^"]*\)".*/\1/p')
identity_version=$(printf '%s\n' "$identity_json" | sed -n 's/.*"schema_version":\([0-9][0-9]*\).*/\1/p')
normalized_rom_sha=$(printf '%s\n' "$identity_json" | sed -n 's/.*"normalized_rom_sha256":"\([0-9a-f]*\)".*/\1/p')
source_byte_order=$(printf '%s\n' "$identity_json" | sed -n 's/.*"source_byte_order":"\([^"]*\)".*/\1/p')
entry_point=$(printf '%s\n' "$identity_json" | sed -n 's/.*"entry_point":\([0-9][0-9]*\).*/\1/p')
[ "$identity_schema" = fn64.rom-identity ] || fail "FN64_ROM_IDENTITY returned the wrong schema"
[ "$identity_version" = 1 ] || fail "FN64_ROM_IDENTITY returned an unsupported schema version"
case "$normalized_rom_sha" in
    *[!0-9a-f]*|'') fail "FN64_ROM_IDENTITY returned an invalid normalized digest" ;;
esac
[ "${#normalized_rom_sha}" -eq 64 ] || fail "FN64_ROM_IDENTITY returned an invalid normalized digest"
[ "$source_byte_order" = "$expected_byte_order" ] || fail "ROM header and FN64_ROM_IDENTITY byte order disagree"
case "$entry_point" in
    *[!0-9]*|'') fail "FN64_ROM_IDENTITY returned an invalid entry point" ;;
esac

rdram_sha=0000000000000000000000000000000000000000000000000000000000000000
if [ -n "$rdram" ]; then
    rdram_size=$(wc -c < "$rdram" | tr -d ' ')
    [ "$rdram_size" -eq 4194304 ] || [ "$rdram_size" -eq 8388608 ] ||
        fail "RDRAM must be exactly 4 MiB or 8 MiB"
    rdram_sha=$(hash_file "$rdram")
fi

ghidra_version=$(awk -F= '$1 == "application.version" { print $2 }' \
    "$ghidra_install/Ghidra/application.properties")
ghidra_release=$(awk -F= '$1 == "application.release.name" { print $2 }' \
    "$ghidra_install/Ghidra/application.properties")
[ -n "$ghidra_version" ] || fail "could not read Ghidra version"
[ -n "$ghidra_release" ] || fail "could not read Ghidra release name"
ghidra_sha=$(hash_file "$ghidra_install/Ghidra/application.properties")
exporter_sha=$(hash_file "$exporter")
install_verifier_sha=$(hash_file "$install_verifier")
runtime_verifier_sha=$(hash_file "$runtime_verifier")
jdk_version=$("$jdk/bin/java" -version 2>&1)
jdk_sha=$(hash_fields fn64.java-runtime.v1 "$jdk_version")
config_sha=$(hash_fields fn64.n64loaderwv-first-contact.config.v2 \
    "$normalized_rom_sha" "$source_byte_order" "$entry_point" "$rdram_sha" \
    "$ghidra_version" "$ghidra_sha" "$jdk_sha" "$loader_repository" \
    "$loader_policy_sha" "$loader_commit" "$loader_tree" "$source_archive_sha" \
    "$extension_sha" "$build_receipt_sha" \
    "$exporter_sha" "$install_verifier_sha" "$runtime_verifier_sha" \
    N64LoaderWVLoader analysis-timeout-seconds=120 max-cpu=1 \
    heap-mib=1024 rss-mib=2048 min-free-percent=40 wall-seconds=180)
request_id=$(hash_fields fn64.n64loaderwv-first-contact.request.v2 \
    "$normalized_rom_sha" "$rdram_sha" "$loader_policy_sha" "$loader_commit" \
    "$extension_sha" "$build_receipt_sha" "$config_sha")

request_root="$workspace/v1/roms/$normalized_rom_sha/runs/n64loaderwv-first-contact/requests/$request_id"
mkdir -p -- "$request_root/attempts"
attempt=$(mktemp -d "$request_root/attempts/attempt.XXXXXXXX") || fail "could not create attempt directory"
chmod 700 "$attempt"
mkdir -m 700 "$attempt/inputs" "$attempt/raw" "$attempt/diagnostics" "$attempt/project" \
    "$attempt/home" "$attempt/tmp" "$attempt/cache" "$attempt/settings" "$attempt/out"

attempt_rom="$attempt/inputs/rom.$expected_byte_order"
rom_source_sha=$(hash_file "$rom")
cp -- "$rom" "$attempt_rom"
[ "$rom_source_sha" = "$(hash_file "$attempt_rom")" ] || fail "ROM changed while copying"
attempt_identity_json=$("$rom_identity" "$attempt_rom") ||
    fail "FN64_ROM_IDENTITY rejected the copied ROM"
[ "$identity_json" = "$attempt_identity_json" ] || fail "ROM identity changed while copying"

attempt_rdram=
if [ -n "$rdram" ]; then
    attempt_rdram="$attempt/inputs/rdram.bin"
    cp -- "$rdram" "$attempt_rdram"
    [ "$rdram_sha" = "$(hash_file "$attempt_rdram")" ] || fail "RDRAM changed while copying"
fi

cp -- "$extension_zip" "$attempt/inputs/n64loaderwv-extension.zip"
installed_zip="$attempt/inputs/n64loaderwv-extension.zip"
[ "$extension_sha" = "$(hash_file "$installed_zip")" ] || fail "extension changed while copying"
cp -- "$build_receipt" "$attempt/inputs/n64loaderwv-conformance-receipt.txt"
installed_build_receipt="$attempt/inputs/n64loaderwv-conformance-receipt.txt"
[ "$build_receipt_sha" = "$(hash_file "$installed_build_receipt")" ] ||
    fail "conformance receipt changed while copying"
"$provenance_verifier" artifact "$artifact_policy" "$policy" \
    "$installed_build_receipt" "$installed_zip" >/dev/null ||
    fail "copied extension artifact failed provenance replay"
archive_entries="$attempt/diagnostics/extension-archive-entries.txt"
unzip -Z1 "$installed_zip" > "$archive_entries"
if grep -Eq '(^/|(^|/)\.\.(/|$))' "$archive_entries"; then
    fail "extension archive contains an unsafe path"
fi
extension_roots=$(awk -F/ 'NF { print $1 }' "$archive_entries" | sort -u)
extension_root_count=$(printf '%s\n' "$extension_roots" | awk 'NF { count++ } END { print count + 0 }')
[ "$extension_root_count" -eq 1 ] || fail "extension archive must have exactly one top-level directory"
extension_root=$(printf '%s\n' "$extension_roots" | awk 'NF { print; exit }')
case "$extension_root" in
    ''|.|..|*[!A-Za-z0-9._-]*) fail "extension archive has an invalid top-level directory" ;;
esac
settings_user="$attempt/home/ghidra/ghidra_${ghidra_version}_${ghidra_release}"
extensions="$settings_user/Extensions"
mkdir -p -- "$extensions"
unzip -q "$installed_zip" -d "$extensions"
[ -f "$extensions/$extension_root/extension.properties" ] ||
    fail "installed extension is missing extension.properties"
extension_dir="$extensions/$extension_root"
install_verification="$attempt/diagnostics/install-verification.json"
"$install_verifier" "$installed_zip" "$extension_dir" "$ghidra_install" \
    "$settings_user" > "$install_verification" ||
    fail "installed extension failed isolated classpath verification"
[ -s "$install_verification" ] || fail "install verifier produced no identity"
install_verification_sha=$(hash_file "$install_verification")
install_field() {
    python3 - "$install_verification" "$1" <<'PY'
import json
import sys

with open(sys.argv[1], "r", encoding="utf-8") as stream:
    value = json.load(stream)
fields = sys.argv[2].split(".")
for field in fields:
    value = value[field]
print(value)
PY
}
[ "$(install_field schema)" = fn64.n64loaderwv-install-verification ] ||
    fail "install verifier returned the wrong schema"
[ "$(install_field schema_version)" = 1 ] ||
    fail "install verifier returned an unsupported schema version"
[ "$(install_field extension_root)" = "$extension_root" ] ||
    fail "install verifier returned the wrong extension root"
loader_jar_sha=$(install_field loader_jar.sha256)
loader_jar_length=$(install_field loader_jar.byte_length)
loader_class_sha=$(install_field loader_class.sha256)
loader_class_length=$(install_field loader_class.byte_length)
case "$loader_jar_sha$loader_class_sha" in
    *[!0-9a-f]*|'') fail "install verifier returned an invalid loader digest" ;;
esac
[ "${#loader_jar_sha}" -eq 64 ] && [ "${#loader_class_sha}" -eq 64 ] ||
    fail "install verifier returned an invalid loader digest"
case "$loader_jar_length:$loader_class_length" in
    *[!0-9:]*|:|*:0|0:*) fail "install verifier returned an invalid loader length" ;;
esac
expected_loader_jar="$extension_dir/lib/$extension_root.jar"
[ -f "$expected_loader_jar" ] && [ ! -L "$expected_loader_jar" ] ||
    fail "approved loader JAR is not the expected regular file"

program_name=$(basename -- "$attempt_rom")
inventory="$attempt/out/review-inventory.json"
runtime_verification="$attempt/out/runtime-verification.json"
analysis_log="$attempt/diagnostics/analyze.log"
guard_log="$attempt/diagnostics/memory-guard.jsonl"
path_value=${PATH:-/usr/bin:/bin}

set +e
if [ -n "$rdram" ]; then
    FN64_GUARD_MAX_RSS_MIB=2048 \
    FN64_GUARD_MIN_FREE_PERCENT=40 \
    FN64_GUARD_MAX_SECONDS=180 \
    FN64_GUARD_JSONL="$guard_log" \
    "$guard" env -i \
        "PATH=$path_value" "HOME=$attempt/home" "TMPDIR=$attempt/tmp" \
        "JAVA_HOME=$jdk" "GHIDRA_HEADLESS_MAXMEM=1G" \
        "_JAVA_OPTIONS=-Dapplication.settingsdir=$attempt/home -Dapplication.cachedir=$attempt/cache -Dapplication.tempdir=$attempt/tmp -Djava.io.tmpdir=$attempt/tmp -Duser.home=$attempt/home" \
        "$headless" "$attempt/project" n64loaderwv-review \
            -import "$attempt_rom" -overwrite \
            -loader N64LoaderWVLoader -loader-rdram "$attempt_rdram" \
            -scriptPath "$repo/tools/ghidra" \
            -analysisTimeoutPerFile 120 -max-cpu 1 \
            -postScript Fn64VerifyN64LoaderRuntime.java \
                "$runtime_verification" "$expected_loader_jar" "$loader_jar_sha" \
                "$loader_class_sha" "N64 Loader by Warranty Voider" \
                "N64 Loader by Warranty Voider" \
            -postScript Fn64ExportReviewInventory.java \
                "$inventory" "$normalized_rom_sha" "$rdram_sha" "$ghidra_version" \
                "$loader_repository" "$loader_commit" "$extension_sha" \
                "$build_receipt_sha" "$config_sha" "$program_name" \
            >"$analysis_log" 2>&1
else
    FN64_GUARD_MAX_RSS_MIB=2048 \
    FN64_GUARD_MIN_FREE_PERCENT=40 \
    FN64_GUARD_MAX_SECONDS=180 \
    FN64_GUARD_JSONL="$guard_log" \
    "$guard" env -i \
        "PATH=$path_value" "HOME=$attempt/home" "TMPDIR=$attempt/tmp" \
        "JAVA_HOME=$jdk" "GHIDRA_HEADLESS_MAXMEM=1G" \
        "_JAVA_OPTIONS=-Dapplication.settingsdir=$attempt/home -Dapplication.cachedir=$attempt/cache -Dapplication.tempdir=$attempt/tmp -Djava.io.tmpdir=$attempt/tmp -Duser.home=$attempt/home" \
        "$headless" "$attempt/project" n64loaderwv-review \
            -import "$attempt_rom" -overwrite \
            -loader N64LoaderWVLoader \
            -scriptPath "$repo/tools/ghidra" \
            -analysisTimeoutPerFile 120 -max-cpu 1 \
            -postScript Fn64VerifyN64LoaderRuntime.java \
                "$runtime_verification" "$expected_loader_jar" "$loader_jar_sha" \
                "$loader_class_sha" "N64 Loader by Warranty Voider" \
                "N64 Loader by Warranty Voider" \
            -postScript Fn64ExportReviewInventory.java \
                "$inventory" "$normalized_rom_sha" "$rdram_sha" "$ghidra_version" \
                "$loader_repository" "$loader_commit" "$extension_sha" \
                "$build_receipt_sha" "$config_sha" "$program_name" \
            >"$analysis_log" 2>&1
fi
analysis_status=$?
set -e
[ "$analysis_status" -eq 0 ] || fail "guarded headless analysis failed; see $analysis_log"
[ -s "$inventory" ] || fail "Fn64ExportReviewInventory.java produced no inventory"
[ -s "$runtime_verification" ] ||
    fail "Fn64VerifyN64LoaderRuntime.java produced no runtime verification"
if grep -q "Ignoring class 'n64loaderwv.N64LoaderWVLoader'" "$analysis_log"; then
    fail "another N64LoaderWV installation shadowed the isolated extension"
fi
grep -q 'Using Loader: N64 Loader by Warranty Voider' "$analysis_log" ||
    fail "headless import did not select N64LoaderWV"
if grep -q 'Using Loader: Raw Binary' "$analysis_log"; then
    fail "headless import unexpectedly selected Raw Binary"
fi

command -v python3 >/dev/null 2>&1 || fail "python3 is required to validate the inventory"
python3 - "$inventory" "$program_name" "$normalized_rom_sha" "$rdram_sha" \
    "$ghidra_version" "$loader_repository" "$loader_commit" "$extension_sha" \
    "$build_receipt_sha" "$config_sha" <<'PY' ||
import json
import sys

path, program, rom_sha, rdram_sha, ghidra_version, loader_repository, loader_commit, extension_sha, build_receipt_sha, config_sha = sys.argv[1:]
with open(path, "r", encoding="utf-8") as stream:
    inventory = json.load(stream)

expected_provenance = {
    "rom_sha256": rom_sha,
    "rdram_sha256": rdram_sha,
    "rdram_present": rdram_sha != "0" * 64,
    "ghidra_version": ghidra_version,
    "loader_repository": loader_repository,
    "loader_commit": loader_commit,
    "extension_sha256": extension_sha,
    "build_receipt_sha256": build_receipt_sha,
    "config_sha256": config_sha,
}
if inventory.get("schema") != "fn64.n64loaderwv-review-inventory.v2":
    raise SystemExit("wrong inventory schema")
if inventory.get("candidate_only") is not True:
    raise SystemExit("inventory is not marked candidate-only")
if inventory.get("program_name") != program:
    raise SystemExit("wrong inventory program name")
if inventory.get("provenance") != expected_provenance:
    raise SystemExit("wrong inventory provenance")
blocks = inventory.get("memory_blocks")
functions = inventory.get("functions")
if not isinstance(blocks, list) or not blocks:
    raise SystemExit("inventory has no memory blocks")
if not isinstance(functions, list) or not functions:
    raise SystemExit("inventory has no functions")
counts = inventory.get("counts")
if not isinstance(counts, dict) or counts.get("memory_blocks") != len(blocks) \
        or counts.get("functions") != len(functions):
    raise SystemExit("inventory counts do not match its arrays")
reachable_count = sum(1 for function in functions
                      if function.get("reachable_from_loader_entry") is True)
if counts.get("reachable_from_loader_entries") != reachable_count:
    raise SystemExit("inventory reachability count does not match function fields")
if any(not isinstance(function.get("body_ranges"), list) or not function["body_ranges"]
       for function in functions):
    raise SystemExit("inventory function has no body ranges")
PY
    fail "invalid review inventory: $inventory"

python3 - "$runtime_verification" "$loader_jar_sha" "$loader_jar_length" \
    "$loader_class_sha" "$loader_class_length" <<'PY' ||
import json
import sys

path, jar_sha, jar_length, class_sha, class_length = sys.argv[1:]
with open(path, "r", encoding="utf-8") as stream:
    value = json.load(stream)
if value.get("schema") != "fn64.n64loaderwv-runtime-verification.v1":
    raise SystemExit("wrong runtime-verification schema")
if value.get("schema_version") != 1:
    raise SystemExit("wrong runtime-verification schema version")
if value.get("loader") != {
    "requested_simple_name": "N64LoaderWVLoader",
    "class_name": "n64loaderwv.N64LoaderWVLoader",
    "display_name": "N64 Loader by Warranty Voider",
}:
    raise SystemExit("wrong runtime loader identity")
runtime = value.get("runtime")
if not isinstance(runtime, dict):
    raise SystemExit("missing runtime loader identity")
if runtime.get("jar_sha256") != jar_sha or runtime.get("jar_byte_length") != int(jar_length):
    raise SystemExit("wrong runtime JAR identity")
if runtime.get("class_sha256") != class_sha or runtime.get("class_byte_length") != int(class_length):
    raise SystemExit("wrong runtime class identity")
if value.get("program") != {"executable_format": "N64 Loader by Warranty Voider"}:
    raise SystemExit("wrong runtime executable format")
PY
    fail "invalid runtime verification: $runtime_verification"

inventory_sha=$(hash_file "$inventory")
runtime_verification_sha=$(hash_file "$runtime_verification")
receipt="$attempt/out/receipt.txt"
{
    printf '%s\n' 'schema=fn64.n64loaderwv-first-contact.receipt.v3'
    printf 'normalized_rom_sha256=%s\n' "$normalized_rom_sha"
    printf 'source_byte_order=%s\n' "$source_byte_order"
    printf 'entry_point=%s\n' "$entry_point"
    printf 'rdram_sha256=%s\n' "$rdram_sha"
    printf 'ghidra_version=%s\n' "$ghidra_version"
    printf 'ghidra_build_sha256=%s\n' "$ghidra_sha"
    printf 'java_runtime_sha256=%s\n' "$jdk_sha"
    printf 'n64loaderwv_repository=%s\n' "$loader_repository"
    printf 'n64loaderwv_policy_sha256=%s\n' "$loader_policy_sha"
    printf 'n64loaderwv_commit=%s\n' "$loader_commit"
    printf 'n64loaderwv_tree=%s\n' "$loader_tree"
    printf 'n64loaderwv_source_archive_sha256=%s\n' "$source_archive_sha"
    printf 'n64loaderwv_extension_sha256=%s\n' "$extension_sha"
    printf 'n64loaderwv_conformance_receipt_sha256=%s\n' "$build_receipt_sha"
    printf 'export_script_sha256=%s\n' "$exporter_sha"
    printf 'install_verifier_sha256=%s\n' "$install_verifier_sha"
    printf 'runtime_verifier_sha256=%s\n' "$runtime_verifier_sha"
    printf 'install_verification_sha256=%s\n' "$install_verification_sha"
    printf 'loader_jar_sha256=%s\n' "$loader_jar_sha"
    printf 'loader_class_sha256=%s\n' "$loader_class_sha"
    printf 'configuration_sha256=%s\n' "$config_sha"
    printf 'request_id=%s\n' "$request_id"
    printf 'review_inventory_sha256=%s\n' "$inventory_sha"
    printf 'runtime_verification_sha256=%s\n' "$runtime_verification_sha"
    printf '%s\n' 'project_retained=true'
} > "$receipt"
chmod 600 "$inventory" "$runtime_verification" "$receipt" "$analysis_log" \
    "$guard_log" "$install_verification"

echo "n64loaderwv first-contact: complete"
echo "attempt=$attempt"
echo "project=$attempt/project"
echo "inventory=$inventory"
echo "receipt=$receipt"
echo "analysis_log=$analysis_log"
echo "memory_guard_log=$guard_log"
