#!/bin/sh
set -eu

repo=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
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
case "$work" in
    "$repo"|"$repo"/*)
        echo "FN64_GHIDRA_WORK must be outside the repository" >&2
        exit 2
        ;;
esac

mkdir -p "$work/inputs" "$work/out" "$work/projects" "$work/home" "$work/cache"
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

    if [ "$mode" = seeded ]; then
        JAVA_HOME="$jdk" \
        _JAVA_OPTIONS="-Duser.home=$work/home -Djava.io.tmpdir=$work/cache" \
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
            -deleteProject >"$work/out/$output.log" 2>&1
    else
        JAVA_HOME="$jdk" \
        _JAVA_OPTIONS="-Duser.home=$work/home -Djava.io.tmpdir=$work/cache" \
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
            -deleteProject >"$work/out/$output.log" 2>&1
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

cargo run --quiet --manifest-path "$repo/Cargo.toml" -p fn64-discover --bin gate_tool_jsonl -- \
    "$work/out/bank-a-unseeded.jsonl" \
    "$work/out/bank-a-seeded.jsonl" \
    "$work/out/bank-b-unseeded.jsonl"

# Stream digests recorded in tools/ghidra/README.md ("Measured conformance").
# They are stable for the pinned Ghidra 12.1.2 build (the JSONL embeds the
# build digest); a different Ghidra build fails here by design.
expected_a_seeded=193ce1641402c9f4c436a7abca70030970859dcb12895535661f863a7fa45e0f
expected_a_unseeded=953c0a01d81dbdd4fb05c9c02d6664c09610254da22970b4246ee9a55892b807
expected_b_unseeded=8d3fdcc8e222d598ea815703296d403a693f192e98639d1ee4657dfa5c5e8e31
for pair in \
    "bank-a-seeded $expected_a_seeded" \
    "bank-a-unseeded $expected_a_unseeded" \
    "bank-b-unseeded $expected_b_unseeded"; do
    stream=${pair% *}
    expected=${pair#* }
    got=$(sha "$work/out/$stream.jsonl")
    if [ "$got" != "$expected" ]; then
        echo "$stream.jsonl sha256 $got != recorded $expected (pinned Ghidra 12.1.2)" >&2
        exit 1
    fi
done

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

echo "ghidra conformance: raw BE MIPS import, bank isolation, and seed lineage passed"
