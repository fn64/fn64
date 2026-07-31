#!/bin/sh
set -eu
umask 077

repo=$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd)
guard="$repo/scripts/memory-guard.zsh"
headless=${GHIDRA_HEADLESS:-/opt/homebrew/Cellar/ghidra/12.1.2/libexec/support/analyzeHeadless}
jdk=${GHIDRA_JAVA_HOME:-/opt/homebrew/Cellar/openjdk@21/21.0.11/libexec/openjdk.jdk/Contents/Home}
work=${FN64_GHIDRA_WORK:-/private/tmp/fn64-ghidra-conformance}

if [ ! -x "$headless" ]; then
    echo "GHIDRA_HEADLESS is not executable: $headless" >&2
    exit 2
fi
if [ ! -x "$jdk/bin/java" ]; then
    echo "GHIDRA_JAVA_HOME does not contain bin/java: $jdk" >&2
    exit 2
fi
if [ ! -x "$guard" ]; then
    echo "repository memory guard is not executable: $guard" >&2
    exit 2
fi
case "$work" in
    "$repo"|"$repo"/*)
        echo "FN64_GHIDRA_WORK must be outside the repository" >&2
        exit 2
        ;;
esac

mkdir -p "$work/inputs" "$work/out" "$work/projects" "$work/home" "$work/settings" \
    "$work/cache" "$work/tmp"
xxd -r -p "$repo/tools/ghidra/fixtures/bank-a.hex" "$work/inputs/bank-a.bin"
xxd -r -p "$repo/tools/ghidra/fixtures/bank-b.hex" "$work/inputs/bank-b.bin"

sha() {
    shasum -a 256 "$1" | awk '{print $1}'
}

bank_a_sha=$(sha "$work/inputs/bank-a.bin")
bank_b_sha=$(sha "$work/inputs/bank-b.bin")
build_sha=$(sha "$(dirname -- "$(dirname -- "$headless")")/Ghidra/application.properties")
seed_script_sha=$(sha "$repo/tools/ghidra/Fn64SeedFunctions.java")
export_script_sha=$(sha "$repo/tools/ghidra/Fn64ExportCandidates.java")
rom_sha=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
evidence_sha=3333333333333333333333333333333333333333333333333333333333333333
snapshot_sha=4444444444444444444444444444444444444444444444444444444444444444
path_value=${PATH:-/usr/bin:/bin}

run_bank() {
    output=$1
    mode=$2
    bank=$3
    input=$4
    bank_sha=$5
    mapping_sha=$6
    extra_seed=$7
    config_sha=$(printf '%s\n%s\n%s\n%s\n%s\n' \
        'MIPS:BE:64:64-32addr:o32' "$mode" "$seed_script_sha" "$export_script_sha" "$mapping_sha" |
        shasum -a 256 | awk '{print $1}')

    run_log="$work/out/$output.log"
    run_guard="$work/out/$output-memory.jsonl"
    if [ "$mode" = seeded ]; then
        FN64_GUARD_MAX_RSS_MIB=2048 \
        FN64_GUARD_MIN_FREE_PERCENT=40 \
        FN64_GUARD_MAX_SECONDS=180 \
        FN64_GUARD_JSONL="$run_guard" \
        "$guard" env -i \
            "PATH=$path_value" "HOME=$work/home" "TMPDIR=$work/tmp" \
            "JAVA_HOME=$jdk" "GHIDRA_HEADLESS_MAXMEM=1G" \
            "_JAVA_OPTIONS=-Dapplication.settingsdir=$work/settings -Dapplication.cachedir=$work/cache -Dapplication.tempdir=$work/tmp -Djava.io.tmpdir=$work/tmp -Duser.home=$work/home" \
            "$headless" "$work/projects" "$output" \
            -import "$input" -overwrite \
            -processor MIPS:BE:64:64-32addr -cspec o32 \
            -loader BinaryLoader -loader-baseAddr 80001000 \
            -scriptPath "$repo/tools/ghidra" \
            -preScript Fn64SeedFunctions.java seeded 0x80001000 0x80001040 0x80001000 "$extra_seed" \
            -analysisTimeoutPerFile 120 -max-cpu 1 \
            -postScript Fn64ExportCandidates.java "$work/out/$output.jsonl" seeded "$bank" \
                0x80001000 0x80001040 "$rom_sha" "$bank_sha" "$mapping_sha" 12.1.2 \
                "$build_sha" "$config_sha" "$evidence_sha" "$(basename -- "$input")" \
                discovery_snapshot "$snapshot_sha" \
            -deleteProject >"$run_log" 2>&1
    else
        FN64_GUARD_MAX_RSS_MIB=2048 \
        FN64_GUARD_MIN_FREE_PERCENT=40 \
        FN64_GUARD_MAX_SECONDS=180 \
        FN64_GUARD_JSONL="$run_guard" \
        "$guard" env -i \
            "PATH=$path_value" "HOME=$work/home" "TMPDIR=$work/tmp" \
            "JAVA_HOME=$jdk" "GHIDRA_HEADLESS_MAXMEM=1G" \
            "_JAVA_OPTIONS=-Dapplication.settingsdir=$work/settings -Dapplication.cachedir=$work/cache -Dapplication.tempdir=$work/tmp -Djava.io.tmpdir=$work/tmp -Duser.home=$work/home" \
            "$headless" "$work/projects" "$output" \
            -import "$input" -overwrite \
            -processor MIPS:BE:64:64-32addr -cspec o32 \
            -loader BinaryLoader -loader-baseAddr 80001000 \
            -scriptPath "$repo/tools/ghidra" \
            -preScript Fn64SeedFunctions.java unseeded 0x80001000 0x80001040 0x80001000 \
            -analysisTimeoutPerFile 120 -max-cpu 1 \
            -postScript Fn64ExportCandidates.java "$work/out/$output.jsonl" unseeded "$bank" \
                0x80001000 0x80001040 "$rom_sha" "$bank_sha" "$mapping_sha" 12.1.2 \
                "$build_sha" "$config_sha" "$evidence_sha" "$(basename -- "$input")" \
                discovery_snapshot "$snapshot_sha" \
            -deleteProject >"$run_log" 2>&1
    fi
}

run_bank bank-a-unseeded unseeded bank-a "$work/inputs/bank-a.bin" "$bank_a_sha" \
    1111111111111111111111111111111111111111111111111111111111111111 ''
run_bank bank-a-seeded seeded bank-a "$work/inputs/bank-a.bin" "$bank_a_sha" \
    1111111111111111111111111111111111111111111111111111111111111111 0x80001030
run_bank bank-b-unseeded unseeded bank-b "$work/inputs/bank-b.bin" "$bank_b_sha" \
    5555555555555555555555555555555555555555555555555555555555555555 ''

for run_log in "$work"/out/*.log; do
    if ! grep -q 'Using Loader: Raw Binary' "$run_log"; then
        echo "$run_log did not use Ghidra's raw binary loader" >&2
        exit 1
    fi
    if ! grep -q 'Using Language/Compiler: MIPS:BE:64:64-32addr:o32' "$run_log"; then
        echo "$run_log did not use the pinned big-endian MIPS/o32 language" >&2
        exit 1
    fi
done

gate_log="$work/out/gate-tool-jsonl.log"
gate_guard="$work/out/gate-tool-jsonl-memory.jsonl"
if [ -n "${FN64_GATE_TOOL_JSONL:-}" ]; then
    case "$FN64_GATE_TOOL_JSONL" in
        /*) ;;
        *) echo "FN64_GATE_TOOL_JSONL must be absolute" >&2; exit 2 ;;
    esac
    if [ ! -x "$FN64_GATE_TOOL_JSONL" ]; then
        echo "FN64_GATE_TOOL_JSONL is not executable: $FN64_GATE_TOOL_JSONL" >&2
        exit 2
    fi
    FN64_GUARD_MAX_RSS_MIB=2048 \
    FN64_GUARD_MIN_FREE_PERCENT=40 \
    FN64_GUARD_MAX_SECONDS=180 \
    FN64_GUARD_JSONL="$gate_guard" \
    "$guard" "$FN64_GATE_TOOL_JSONL" \
        "$work/out/bank-a-unseeded.jsonl" \
        "$work/out/bank-a-seeded.jsonl" \
        "$work/out/bank-b-unseeded.jsonl" >"$gate_log" 2>&1
else
    caller_home=${HOME:-}
    if [ -z "$caller_home" ]; then
        echo "HOME or FN64_GATE_TOOL_JSONL is required for the Rust gate" >&2
        exit 2
    fi
    cargo_home=${CARGO_HOME:-$caller_home/.cargo}
    rustup_home=${RUSTUP_HOME:-$caller_home/.rustup}
    cargo_target=${CARGO_TARGET_DIR:-$repo/target}
    FN64_GUARD_MAX_RSS_MIB=2048 \
    FN64_GUARD_MIN_FREE_PERCENT=40 \
    FN64_GUARD_MAX_SECONDS=180 \
    FN64_GUARD_JSONL="$gate_guard" \
    "$guard" env -i \
        "PATH=$path_value" "HOME=$work/home" "TMPDIR=$work/tmp" \
        "CARGO_HOME=$cargo_home" "RUSTUP_HOME=$rustup_home" \
        "CARGO_TARGET_DIR=$cargo_target" "CARGO_BUILD_JOBS=1" \
        cargo run --quiet --manifest-path "$repo/Cargo.toml" -j 1 -p fn64-discover \
            --bin gate_tool_jsonl -- \
            "$work/out/bank-a-unseeded.jsonl" \
            "$work/out/bank-a-seeded.jsonl" \
            "$work/out/bank-b-unseeded.jsonl" >"$gate_log" 2>&1
fi

# Snapshot lineage changes every stream. Record one-run digests here, but do
# not promote them to the README's ten-run deterministic claim until ten
# consecutive clean guarded runs agree.
a_seeded_sha=$(sha "$work/out/bank-a-seeded.jsonl")
a_unseeded_sha=$(sha "$work/out/bank-a-unseeded.jsonl")
b_unseeded_sha=$(sha "$work/out/bank-b-unseeded.jsonl")
expected_a_seeded=062377050bbabfd7ad34e8f968608cee83e859342a8af22c5ff3fe88d9b6bc08
expected_a_unseeded=9beaec498da4c35af821ea662dec1e46a8b532f3084ca248e0a7330eba51e4e6
expected_b_unseeded=748246baa3b4bd9fadc3466a6926f3156894c7eb636666e3bcc9661f47099461
[ "$a_seeded_sha" = "$expected_a_seeded" ] || {
    echo "bank A seeded digest drifted: $a_seeded_sha" >&2
    exit 1
}
[ "$a_unseeded_sha" = "$expected_a_unseeded" ] || {
    echo "bank A unseeded digest drifted: $a_unseeded_sha" >&2
    exit 1
}
[ "$b_unseeded_sha" = "$expected_b_unseeded" ] || {
    echo "bank B unseeded digest drifted: $b_unseeded_sha" >&2
    exit 1
}

if ! grep -q 'ghidra:function-entry:bank-a:80001020' "$work/out/bank-a-unseeded.jsonl"; then
    echo "bank A did not discover its direct-call target" >&2
    exit 1
fi
if grep -q 'ghidra:function-entry:bank-a:80001030' "$work/out/bank-a-unseeded.jsonl"; then
    echo "unseeded bank A unexpectedly contains the seeded-only entry" >&2
    exit 1
fi
if ! grep -q 'ghidra:function-entry:bank-a:80001030' "$work/out/bank-a-seeded.jsonl"; then
    echo "seeded bank A did not preserve its explicit seed lineage" >&2
    exit 1
fi
if ! grep -q 'ghidra:function-entry:bank-b:80001030' "$work/out/bank-b-unseeded.jsonl"; then
    echo "bank B did not discover its direct-call target" >&2
    exit 1
fi
if grep -q 'ghidra:function-entry:bank-a:' "$work/out/bank-b-unseeded.jsonl"; then
    echo "same-VA bank identity leaked across isolated projects" >&2
    exit 1
fi

echo "ghidra conformance: raw BE MIPS import, bank isolation, and snapshot-bound seed modes passed"
echo "one-run digests (ten-run refresh pending): bank-a-seeded=$a_seeded_sha bank-a-unseeded=$a_unseeded_sha bank-b-unseeded=$b_unseeded_sha"
