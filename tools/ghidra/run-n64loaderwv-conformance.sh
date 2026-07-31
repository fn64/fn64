#!/bin/sh
set -eu
umask 077

repo=$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd)
guard="$repo/scripts/memory-guard.zsh"
policy="$repo/tools/ghidra/n64loaderwv-source-policy.json"
provenance_verifier="$repo/tools/ghidra/verify-n64loaderwv-provenance.py"
launcher_verifier="$repo/tools/ghidra/verify-ghidra-launcher.py"
install_verifier="$repo/tools/ghidra/verify-n64loaderwv-install.py"
runtime_verifier="$repo/tools/ghidra/Fn64VerifyN64LoaderRuntime.java"
approved_repository=https://github.com/fn64/N64LoaderWV

fail() {
    echo "n64loaderwv conformance: $1" >&2
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

checkout=${N64LOADERWV_CHECKOUT:-}
commit=${N64LOADERWV_COMMIT:-}
work=${FN64_GHIDRA_WORK:-}
jdk=${GHIDRA_JAVA_HOME:-}

[ -n "$checkout" ] || fail "N64LOADERWV_CHECKOUT is required"
[ -n "$commit" ] || fail "N64LOADERWV_COMMIT is required"
[ -n "$work" ] || fail "FN64_GHIDRA_WORK is required"
[ -n "$jdk" ] || fail "GHIDRA_JAVA_HOME is required"
case "$checkout" in
    /*) ;;
    *) fail "N64LOADERWV_CHECKOUT must be absolute" ;;
esac
case "$work" in
    /*) ;;
    *) fail "FN64_GHIDRA_WORK must be absolute" ;;
esac
case "$commit" in
    *[!0-9a-f]*|'') fail "N64LOADERWV_COMMIT must be a full lowercase commit SHA" ;;
esac
[ "${#commit}" -eq 40 ] || fail "N64LOADERWV_COMMIT must be a full lowercase commit SHA"
[ -e "$checkout/.git" ] || fail "N64LOADERWV_CHECKOUT must name a Git checkout"
[ ! -L "$checkout" ] || fail "N64LOADERWV_CHECKOUT must not be a symlink"
checkout=$(CDPATH='' cd -- "$checkout" && pwd -P) || fail "could not resolve N64LOADERWV_CHECKOUT"
case "$checkout" in
    "$repo"|"$repo"/*) fail "N64LOADERWV_CHECKOUT must be outside the fn64 repository" ;;
esac
[ -x "$provenance_verifier" ] || fail "N64LoaderWV provenance verifier is not executable"
[ -x "$install_verifier" ] || fail "N64LoaderWV install verifier is not executable"
[ -f "$policy" ] || fail "N64LoaderWV source policy is missing"
"$provenance_verifier" checkout "$policy" "$checkout" "$commit" >/dev/null ||
    fail "N64LOADERWV_CHECKOUT does not satisfy the fn64 source policy"
conformance_mode=approved
policy_sha=$(hash_file "$policy")
approved_tree=$(git -C "$checkout" rev-parse --verify "$commit^{tree}")
[ -x "$jdk/bin/java" ] || fail "GHIDRA_JAVA_HOME does not contain bin/java"
[ -x "$guard" ] || fail "repository memory guard is not executable"
[ -f "$repo/tools/ghidra/Fn64ExportN64LoaderCandidates.java" ] ||
    fail "Fn64ExportN64LoaderCandidates.java is missing"
[ -f "$runtime_verifier" ] || fail "Fn64VerifyN64LoaderRuntime.java is missing"
[ -f "$repo/tools/ghidra/fixtures/bank-a.hex" ] || fail "bank-a fixture is missing"

resolved_commit=$(git -C "$checkout" rev-parse --verify "$commit^{commit}" 2>/dev/null) ||
    fail "N64LOADERWV_COMMIT is not a commit in N64LOADERWV_CHECKOUT"
[ "$resolved_commit" = "$commit" ] || fail "N64LOADERWV_COMMIT did not resolve to itself"

if [ -n "${GHIDRA_INSTALL_DIR:-}" ]; then
    ghidra_install=$GHIDRA_INSTALL_DIR
    case "$ghidra_install" in
        /*) ;;
        *) fail "GHIDRA_INSTALL_DIR must be absolute" ;;
    esac
    headless=${GHIDRA_HEADLESS:-$ghidra_install/support/analyzeHeadless}
else
    headless=${GHIDRA_HEADLESS:-}
    [ -n "$headless" ] || fail "GHIDRA_INSTALL_DIR or GHIDRA_HEADLESS is required"
    case "$headless" in
        /*) ;;
        *) fail "GHIDRA_HEADLESS must be absolute" ;;
    esac
    ghidra_install=$(CDPATH='' cd -- "$(dirname -- "$headless")/.." && pwd) ||
        fail "could not derive GHIDRA_INSTALL_DIR from GHIDRA_HEADLESS"
fi
[ -x "$headless" ] || fail "GHIDRA_HEADLESS is not executable"
[ -x "$launcher_verifier" ] || fail "Ghidra launcher verifier is not executable"
"$launcher_verifier" "$ghidra_install" "$headless" ||
    fail "GHIDRA_HEADLESS does not belong to GHIDRA_INSTALL_DIR"
[ -f "$ghidra_install/Ghidra/application.properties" ] ||
    fail "Ghidra application.properties is missing"
if [ ! -e "$work" ]; then
    mkdir -m 700 -- "$work"
fi
[ -d "$work" ] || fail "FN64_GHIDRA_WORK is not a directory"
[ ! -L "$work" ] || fail "FN64_GHIDRA_WORK must not be a symlink"
work=$(CDPATH='' cd -- "$work" && pwd)
case "$work" in
    "$repo"|"$repo"/*) fail "FN64_GHIDRA_WORK must be outside the repository" ;;
esac
if workspace_mode=$(stat -f '%Lp' "$work" 2>/dev/null); then
    :
elif workspace_mode=$(stat -c '%a' "$work" 2>/dev/null); then
    :
else
    fail "could not inspect FN64_GHIDRA_WORK permissions"
fi
[ "$workspace_mode" = 700 ] || fail "FN64_GHIDRA_WORK must have mode 0700"

attempt=$(mktemp -d "$work/attempt.XXXXXXXX") || fail "could not create an attempt directory"
chmod 700 "$attempt"
mkdir -m 700 "$attempt/build" "$attempt/inputs" "$attempt/out" "$attempt/projects" \
    "$attempt/home" "$attempt/tmp" "$attempt/cache"
build_repo="$attempt/build/N64LoaderWV"
mkdir -m 700 "$build_repo"

# Build only the requested immutable commit; the checkout's worktree and
# untracked files are neither read by Gradle nor copied into the attempt.
source_archive="$attempt/build/n64loaderwv-source.tar"
git -C "$checkout" archive --format=tar --output="$source_archive" "$commit"
tar -xf "$source_archive" -C "$build_repo"
source_archive_sha=$(hash_file "$source_archive")
printf '%s\n' "$commit" > "$attempt/build/n64loaderwv-commit.txt"

path_value=${PATH:-/usr/bin:/bin}
build_log="$attempt/out/build.log"
build_guard="$attempt/out/build-memory.jsonl"
set +e
FN64_GUARD_MAX_RSS_MIB=2048 \
FN64_GUARD_MIN_FREE_PERCENT=40 \
FN64_GUARD_MAX_SECONDS=180 \
FN64_GUARD_JSONL="$build_guard" \
"$guard" env -i \
    "PATH=$path_value" "HOME=$attempt/home" "TMPDIR=$attempt/tmp" \
    "JAVA_HOME=$jdk" "GRADLE_USER_HOME=$attempt/cache/gradle" \
    "_JAVA_OPTIONS=-Dapplication.settingsdir=$attempt/home -Dapplication.cachedir=$attempt/cache -Dapplication.tempdir=$attempt/tmp -Djava.io.tmpdir=$attempt/tmp -Duser.home=$attempt/home" \
    "GRADLE_OPTS=-Xmx1024m" \
    gradle --no-daemon -PGHIDRA_INSTALL_DIR="$ghidra_install" \
        -p "$build_repo" clean check buildExtension >"$build_log" 2>&1
build_status=$?
set -e
[ "$build_status" -eq 0 ] || fail "guarded extension build failed; see $build_log"

artifact_list="$attempt/build/artifacts.txt"
find "$build_repo/dist" -type f -name '*.zip' -print > "$artifact_list"
artifact_count=$(wc -l < "$artifact_list" | tr -d ' ')
[ "$artifact_count" -eq 1 ] || fail "expected exactly one built extension archive"
IFS= read -r artifact < "$artifact_list"
artifact_sha=$(hash_file "$artifact")
cp "$artifact" "$attempt/build/n64loaderwv-extension.zip"
artifact="$attempt/build/n64loaderwv-extension.zip"
[ "$artifact_sha" = "$(hash_file "$artifact")" ] || fail "extension archive changed while copying"

ghidra_version=$(awk -F= '$1 == "application.version" { print $2 }' \
    "$ghidra_install/Ghidra/application.properties")
ghidra_release=$(awk -F= '$1 == "application.release.name" { print $2 }' \
    "$ghidra_install/Ghidra/application.properties")
[ -n "$ghidra_version" ] || fail "could not read Ghidra version"
[ -n "$ghidra_release" ] || fail "could not read Ghidra release name"
settings_user="$attempt/home/ghidra/ghidra_${ghidra_version}_${ghidra_release}"
extensions="$settings_user/Extensions"
mkdir -p "$extensions"

archive_entries="$attempt/build/archive-entries.txt"
unzip -Z1 "$artifact" > "$archive_entries"
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
unzip -q "$artifact" -d "$extensions"
[ -f "$extensions/$extension_root/extension.properties" ] ||
    fail "installed extension is missing extension.properties"
[ -f "$extensions/$extension_root/lib/$extension_root.jar" ] ||
    fail "installed extension is missing its module JAR"
install_receipt="$attempt/out/install-verification.json"
"$install_verifier" "$artifact" "$extensions/$extension_root" \
    "$ghidra_install" "$settings_user" > "$install_receipt" ||
    fail "installed extension does not exactly match the approved fn64 fork"
loader_jar_sha=$(python3 -c \
    'import json,sys; print(json.load(open(sys.argv[1], encoding="utf-8"))["loader_jar"]["sha256"])' \
    "$install_receipt")
loader_class_sha=$(python3 -c \
    'import json,sys; print(json.load(open(sys.argv[1], encoding="utf-8"))["loader_class"]["sha256"])' \
    "$install_receipt")
loader_jar="$extensions/$extension_root/lib/$extension_root.jar"

rom="$attempt/inputs/synthetic.z64"
rdram="$attempt/inputs/rdram-4m.bin"
bank="$attempt/inputs/bank-a.bin"
dd if=/dev/zero of="$rom" bs=8192 count=1 2>/dev/null
printf '\200\067\022\100' | dd of="$rom" bs=1 seek=0 conv=notrunc 2>/dev/null
printf '\200\000\020\000' | dd of="$rom" bs=1 seek=8 conv=notrunc 2>/dev/null
xxd -r -p "$repo/tools/ghidra/fixtures/bank-a.hex" > "$bank"
dd if=/dev/zero of="$rdram" bs=1048576 count=4 2>/dev/null
dd if="$bank" of="$rdram" bs=1 seek=4096 conv=notrunc 2>/dev/null
chmod 600 "$rom" "$rdram" "$bank"
[ "$(wc -c < "$rom" | tr -d ' ')" -eq 8192 ] || fail "synthetic ROM has the wrong size"
[ "$(wc -c < "$rdram" | tr -d ' ')" -eq 4194304 ] || fail "synthetic RDRAM has the wrong size"
embedded_bank="$attempt/inputs/embedded-bank-a.bin"
dd if="$rdram" of="$embedded_bank" bs=1 skip=4096 count=64 2>/dev/null
cmp -s "$bank" "$embedded_bank" || fail "bank-a was not embedded at RDRAM offset 0x1000"

rom_sha=$(hash_file "$rom")
bank_sha=$(hash_file "$bank")
build_sha=$(hash_file "$ghidra_install/Ghidra/application.properties")
export_sha=$(hash_file "$repo/tools/ghidra/Fn64ExportN64LoaderCandidates.java")
install_verifier_sha=$(hash_file "$install_verifier")
runtime_verifier_sha=$(hash_file "$runtime_verifier")
mapping_sha=$(hash_fields fn64.n64loaderwv.mapping.v1 bank-a 80001000 80001040 00001000 "$bank_sha")
config_sha=$(hash_fields fn64.n64loaderwv.config.v1 "$ghidra_version" \
    "$build_sha" MIPS:BE:64:64-32addr:o32 "$approved_repository" "$policy_sha" \
    "$commit" "$approved_tree" "$source_archive_sha" "$artifact_sha" "$export_sha" \
    "$install_verifier_sha" "$runtime_verifier_sha" "$loader_jar_sha" "$loader_class_sha" \
    N64LoaderWVLoader loader-rdram analysisTimeoutPerFile=120 max-cpu=1 heap=1G)
evidence_sha=$(hash_fields fn64.n64loaderwv.evidence.v1 "$rom_sha" "$(hash_file "$rdram")" \
    "$bank_sha" "$mapping_sha" "$approved_repository" "$policy_sha" "$commit" \
    "$approved_tree" "$source_archive_sha" "$artifact_sha" "$loader_jar_sha" \
    "$loader_class_sha")

jsonl="$attempt/out/n64loaderwv-bank-a.jsonl"
runtime_receipt="$attempt/out/runtime-verification.json"
analysis_log="$attempt/out/analyze.log"
analysis_guard="$attempt/out/analyze-memory.jsonl"
program_name=$(basename -- "$rom")
set +e
FN64_GUARD_MAX_RSS_MIB=2048 \
FN64_GUARD_MIN_FREE_PERCENT=40 \
FN64_GUARD_MAX_SECONDS=180 \
FN64_GUARD_JSONL="$analysis_guard" \
"$guard" env -i \
    "PATH=$path_value" "HOME=$attempt/home" "TMPDIR=$attempt/tmp" \
    "JAVA_HOME=$jdk" "GHIDRA_HEADLESS_MAXMEM=1G" \
    "_JAVA_OPTIONS=-Dapplication.settingsdir=$attempt/home -Dapplication.cachedir=$attempt/cache -Dapplication.tempdir=$attempt/tmp -Djava.io.tmpdir=$attempt/tmp -Duser.home=$attempt/home" \
    "$headless" "$attempt/projects" n64loaderwv-conformance \
        -import "$rom" -overwrite \
        -loader N64LoaderWVLoader -loader-rdram "$rdram" \
        -scriptPath "$repo/tools/ghidra" \
        -analysisTimeoutPerFile 120 -max-cpu 1 \
        -postScript Fn64ExportN64LoaderCandidates.java \
            "$jsonl" bank-a 0x80001000 0x80001040 \
            "$rom_sha" "$bank_sha" "$mapping_sha" "$commit" \
            "$artifact_sha" "$config_sha" "$evidence_sha" "$program_name" \
        -postScript Fn64VerifyN64LoaderRuntime.java \
            "$runtime_receipt" "$loader_jar" "$loader_jar_sha" "$loader_class_sha" \
            'N64 Loader by Warranty Voider' 'N64 Loader by Warranty Voider' \
        -deleteProject >"$analysis_log" 2>&1
analysis_status=$?
set -e
[ "$analysis_status" -eq 0 ] || fail "guarded headless analysis failed; see $analysis_log"
[ -s "$jsonl" ] || fail "Ghidra exporter did not produce JSONL"
[ -s "$runtime_receipt" ] || fail "Ghidra did not verify the loaded N64LoaderWV runtime"
if grep -q "Ignoring class 'n64loaderwv.N64LoaderWVLoader'" "$analysis_log"; then
    fail "another N64LoaderWV installation shadowed the isolated extension"
fi
grep -q 'Using Loader: N64 Loader by Warranty Voider' "$analysis_log" ||
    fail "headless import did not select N64LoaderWV"
if grep -q 'Using Loader: Raw Binary' "$analysis_log"; then
    fail "headless import unexpectedly selected Raw Binary"
fi

gate_log="$attempt/out/gate-tool-jsonl.log"
gate_guard="$attempt/out/gate-memory.jsonl"
set +e
if [ -n "${FN64_GATE_TOOL_JSONL:-}" ]; then
    case "$FN64_GATE_TOOL_JSONL" in
        /*) ;;
        *) fail "FN64_GATE_TOOL_JSONL must be absolute" ;;
    esac
    [ -x "$FN64_GATE_TOOL_JSONL" ] || fail "FN64_GATE_TOOL_JSONL is not executable"
    FN64_GUARD_MAX_RSS_MIB=2048 \
    FN64_GUARD_MIN_FREE_PERCENT=40 \
    FN64_GUARD_MAX_SECONDS=180 \
    FN64_GUARD_JSONL="$gate_guard" \
    "$guard" "$FN64_GATE_TOOL_JSONL" "$jsonl" >"$gate_log" 2>&1
else
    caller_home=${HOME:-}
    [ -n "$caller_home" ] || fail "HOME or FN64_GATE_TOOL_JSONL is required for the Rust gate"
    cargo_home=${CARGO_HOME:-$caller_home/.cargo}
    rustup_home=${RUSTUP_HOME:-$caller_home/.rustup}
    cargo_target=${CARGO_TARGET_DIR:-$repo/target}
    FN64_GUARD_MAX_RSS_MIB=2048 \
    FN64_GUARD_MIN_FREE_PERCENT=40 \
    FN64_GUARD_MAX_SECONDS=180 \
    FN64_GUARD_JSONL="$gate_guard" \
    "$guard" env -i \
        "PATH=$path_value" "HOME=$attempt/home" "TMPDIR=$attempt/tmp" \
        "CARGO_HOME=$cargo_home" "RUSTUP_HOME=$rustup_home" \
        "CARGO_TARGET_DIR=$cargo_target" "CARGO_BUILD_JOBS=1" \
        cargo run --quiet --manifest-path "$repo/Cargo.toml" -j 1 -p fn64-discover \
            --bin gate_tool_jsonl -- "$jsonl" >"$gate_log" 2>&1
fi
gate_status=$?
set -e
[ "$gate_status" -eq 0 ] || fail "gate_tool_jsonl rejected the provider stream; see $gate_log"
grep -q 'n64loaderwv:function-entry:bank-a:80001020' "$jsonl" ||
    fail "Ghidra did not export bank-a's direct-call target"

if find "$attempt/projects" -mindepth 1 -print -quit | grep -q .; then
    fail "Ghidra project was not deleted"
fi

jsonl_sha=$(hash_file "$jsonl")
receipt="$attempt/out/receipt.txt"
{
    printf '%s\n' 'schema=fn64.n64loaderwv-conformance.v2'
    printf 'conformance_mode=%s\n' "$conformance_mode"
    printf 'n64loaderwv_repository=%s\n' "$approved_repository"
    printf 'n64loaderwv_policy_sha256=%s\n' "$policy_sha"
    printf 'n64loaderwv_commit=%s\n' "$commit"
    printf 'n64loaderwv_tree=%s\n' "$approved_tree"
    printf 'n64loaderwv_source_archive_sha256=%s\n' "$source_archive_sha"
    printf 'n64loaderwv_extension_sha256=%s\n' "$artifact_sha"
    printf 'ghidra_version=%s\n' "$ghidra_version"
    printf 'ghidra_build_sha256=%s\n' "$build_sha"
    printf 'export_script_sha256=%s\n' "$export_sha"
    printf 'rom_sha256=%s\n' "$rom_sha"
    printf 'rdram_sha256=%s\n' "$(hash_file "$rdram")"
    printf 'bank_sha256=%s\n' "$bank_sha"
    printf 'mapping_sha256=%s\n' "$mapping_sha"
    printf 'configuration_sha256=%s\n' "$config_sha"
    printf 'evidence_sha256=%s\n' "$evidence_sha"
    printf 'provider_jsonl_sha256=%s\n' "$jsonl_sha"
    printf 'build_memory_guard_sha256=%s\n' "$(hash_file "$build_guard")"
    printf 'analysis_memory_guard_sha256=%s\n' "$(hash_file "$analysis_guard")"
    printf 'gate_memory_guard_sha256=%s\n' "$(hash_file "$gate_guard")"
} > "$receipt"
"$provenance_verifier" candidate-integrity "$receipt" "$artifact" >/dev/null ||
    fail "generated conformance receipt failed integrity replay"

echo "n64loaderwv conformance: passed attempt=$attempt extension_sha256=$artifact_sha jsonl_sha256=$jsonl_sha"
