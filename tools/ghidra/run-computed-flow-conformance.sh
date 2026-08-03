#!/bin/sh
set -eu
umask 077

repo=$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd)
guard="$repo/scripts/memory-guard.zsh"
exporter="$repo/tools/ghidra/Fn64ExportComputedFlows.java"
seeder="$repo/tools/ghidra/Fn64SeedComputedFlowFixture.java"
fixture="$repo/tools/ghidra/fixtures/computed-flow.hex"
launcher_verifier="$repo/tools/ghidra/verify-ghidra-launcher.py"
work=${FN64_GHIDRA_WORK:-/private/tmp/fn64-ghidra-computed-flow-conformance}

fail() {
    echo "computed-flow conformance: $1" >&2
    exit 2
}

hash_file() {
    shasum -a 256 -- "$1" | awk '{print $1}'
}

hash_fields() {
    printf '%s\n' "$@" | shasum -a 256 | awk '{print $1}'
}

ghidra_install=${GHIDRA_INSTALL_DIR:-}
jdk=${GHIDRA_JAVA_HOME:-}
[ -n "$ghidra_install" ] || fail "GHIDRA_INSTALL_DIR is required"
[ -n "$jdk" ] || fail "GHIDRA_JAVA_HOME is required"
case "$ghidra_install:$jdk:$work" in
    /*:/*:/*) ;;
    *) fail "GHIDRA_INSTALL_DIR, GHIDRA_JAVA_HOME, and FN64_GHIDRA_WORK must be absolute" ;;
esac
case "$work" in
    "$repo"|"$repo"/*) fail "FN64_GHIDRA_WORK must remain outside the repository" ;;
esac
headless="$ghidra_install/support/analyzeHeadless"
[ -x "$headless" ] || fail "selected analyzeHeadless is not executable"
[ -x "$jdk/bin/java" ] || fail "GHIDRA_JAVA_HOME does not contain bin/java"
[ -x "$guard" ] || fail "memory guard is not executable"
[ -f "$exporter" ] && [ -f "$seeder" ] && [ -f "$fixture" ] ||
    fail "computed-flow fixture sources are incomplete"
"$launcher_verifier" "$ghidra_install" "$headless" ||
    fail "analyzeHeadless does not belong to GHIDRA_INSTALL_DIR"

if [ ! -e "$work" ]; then
    mkdir -m 700 "$work"
fi
[ -d "$work" ] && [ ! -L "$work" ] || fail "FN64_GHIDRA_WORK must be a directory"
if work_mode=$(stat -c '%a' "$work" 2>/dev/null); then :
elif work_mode=$(stat -f '%Lp' "$work" 2>/dev/null); then :
else fail "could not inspect FN64_GHIDRA_WORK permissions"
fi
[ "$work_mode" = 700 ] || fail "FN64_GHIDRA_WORK must have mode 0700"
work=$(CDPATH='' cd -- "$work" && pwd -P)

attempt=$(mktemp -d "$work/attempt.XXXXXXXX") || fail "could not create attempt"
chmod 700 "$attempt"
mkdir -m 700 "$attempt/project" "$attempt/home" "$attempt/settings" \
    "$attempt/cache" "$attempt/tmp" "$attempt/out" "$attempt/tool-artifacts"
cp -- "$exporter" "$attempt/tool-artifacts/Fn64ExportComputedFlows.java"
cp -- "$seeder" "$attempt/tool-artifacts/Fn64SeedComputedFlowFixture.java"
cp -- "$fixture" "$attempt/tool-artifacts/computed-flow.hex"
chmod 600 "$attempt/tool-artifacts"/*
xxd -r -p "$attempt/tool-artifacts/computed-flow.hex" "$attempt/computed-flow.bin"
chmod 600 "$attempt/computed-flow.bin"

fixture_sha=$(hash_file "$attempt/computed-flow.bin")
[ "$fixture_sha" = 019bbd2366bdfb58f9e5104e97aca12736fa5f3be8937a21ae9208239bb5a6bf ] ||
    fail "handwritten fixture digest drifted"
exporter_sha=$(hash_file "$attempt/tool-artifacts/Fn64ExportComputedFlows.java")
seeder_sha=$(hash_file "$attempt/tool-artifacts/Fn64SeedComputedFlowFixture.java")
properties="$ghidra_install/Ghidra/application.properties"
ghidra_version=$(awk -F= '$1 == "application.version" { print $2 }' "$properties")
[ -n "$ghidra_version" ] || fail "could not read Ghidra version"
properties_sha=$(hash_file "$properties")
tool_sha=$(hash_fields fn64.ghidra-computed-flow-tool.v1 \
    "$properties_sha" "$exporter_sha" "$seeder_sha")
mapping_sha=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
config_sha=$(hash_fields fn64.ghidra-computed-flow-config.v1 \
    MIPS:BE:64:64-32addr:o32 discovery_only "$mapping_sha" "$tool_sha")
evidence_sha=$(hash_fields fn64.ghidra-computed-flow-evidence.v1 \
    "$fixture_sha" "$seeder_sha")
snapshot_sha=ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff
rom_sha=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
output="$attempt/out/computed-flows.jsonl"
analysis_log="$attempt/out/analyze.log"
guard_log="$attempt/out/memory.jsonl"
path_value=${PATH:-/usr/bin:/bin}

FN64_GUARD_MAX_RSS_MIB=2048 \
FN64_GUARD_MIN_FREE_PERCENT=40 \
FN64_GUARD_MAX_SECONDS=180 \
FN64_GUARD_JSONL="$guard_log" \
"$guard" env -i "PATH=$path_value" "HOME=$attempt/home" "TMPDIR=$attempt/tmp" \
    "JAVA_HOME=$jdk" "GHIDRA_HEADLESS_MAXMEM=1G" \
    "_JAVA_OPTIONS=-Dapplication.settingsdir=$attempt/settings -Dapplication.cachedir=$attempt/cache -Dapplication.tempdir=$attempt/tmp -Djava.io.tmpdir=$attempt/tmp -Duser.home=$attempt/home" \
    "$headless" "$attempt/project" computed-flow-conformance \
        -import "$attempt/computed-flow.bin" -overwrite \
        -processor MIPS:BE:64:64-32addr -cspec o32 \
        -loader BinaryLoader -loader-baseAddr 80001000 \
        -scriptPath "$attempt/tool-artifacts" \
        -preScript Fn64SeedComputedFlowFixture.java \
        -analysisTimeoutPerFile 120 -max-cpu 1 \
        -postScript Fn64ExportComputedFlows.java "$output" discovery_only fixture \
            0x80001000 0x800011a8 "$rom_sha" "$fixture_sha" "$mapping_sha" \
            "$ghidra_version" "$tool_sha" "$config_sha" "$evidence_sha" \
            computed-flow.bin discovery_snapshot "$snapshot_sha" \
        -deleteProject >"$analysis_log" 2>&1 ||
    fail "Ghidra analysis failed; see $analysis_log"

[ -s "$output" ] || fail "Ghidra exporter produced no JSONL"
grep -q 'Using Loader: Raw Binary' "$analysis_log" || fail "wrong Ghidra loader"
grep -q 'Using Language/Compiler: MIPS:BE:64:64-32addr:o32' "$analysis_log" ||
    fail "wrong Ghidra language/compiler"

gate=${FN64_GATE_TOOL_JSONL:-$repo/target/debug/gate_tool_jsonl}
case "$gate" in /*) ;; *) fail "FN64_GATE_TOOL_JSONL must be absolute" ;; esac
[ -x "$gate" ] || fail "build gate_tool_jsonl or set FN64_GATE_TOOL_JSONL"
"$gate" "$output" > "$attempt/out/gate.log" 2>&1 ||
    fail "strict Rust schema gate rejected the computed-flow stream"

python3 - "$output" <<'PY' || fail "computed-flow semantics changed"
import json, sys
records = [json.loads(line) for line in open(sys.argv[1], encoding="utf-8")]
header, *middle, summary = records
assert header["schema"] == "fn64.tool-adapter" and header["schema_version"] == 3
assert header["role"] == "control_flow_candidates"
actual = []
for record in middle:
    claim = record["claim"]
    assert claim["type"] == "computed_control_flow"
    assert claim["completeness"] == "unknown"
    actual.append((claim["site"]["pc"], claim["via_call"], [item["pc"] for item in claim["targets"]]))
expected = [
    (0x80001008, True, [0x80001180]),
    (0x80001028, True, [0x800011a0]),
    (0x80001048, True, [0x80001160]),
    (0x80001084, False, [0x800010b0, 0x800010c0, 0x800010d0]),
    (0x80001120, False, []),
]
assert actual == expected, (actual, expected)
assert not any(site in {0x80001160, 0x80001180, 0x800011a0} for site, _, _ in actual)
assert summary["claim_records"] == len(expected)
assert summary["claims_sha256"] == "c42f83a14ccd7358b8c9292dc76478f9225a8cd717763fac9f71fa5add4de25c"
assert summary["resources"]["warnings"] == []
PY

[ "$(hash_file "$exporter")" = "$exporter_sha" ] || fail "exporter changed during analysis"
[ "$(hash_file "$seeder")" = "$seeder_sha" ] || fail "seeder changed during analysis"
[ "$(hash_file "$properties")" = "$properties_sha" ] || fail "Ghidra changed during analysis"
output_sha=$(hash_file "$output")
python3 - "$attempt/out/receipt.json" "$ghidra_version" "$properties_sha" \
    "$fixture_sha" "$exporter_sha" "$seeder_sha" "$output_sha" <<'PY'
import json, sys
path, version, distribution, fixture, exporter, seeder, output = sys.argv[1:]
value = {
    "candidate_only": True,
    "computed_flow_claims": 5,
    "computed_flow_claims_sha256": "c42f83a14ccd7358b8c9292dc76478f9225a8cd717763fac9f71fa5add4de25c",
    "exporter_sha256": exporter,
    "fixture_sha256": fixture,
    "ghidra_application_properties_sha256": distribution,
    "ghidra_version": version,
    "provider_jsonl_sha256": output,
    "schema": "fn64.ghidra-computed-flow-conformance",
    "schema_version": 1,
    "seeder_sha256": seeder,
    "target_set_completeness": "unknown",
}
with open(path, "x", encoding="utf-8") as stream:
    stream.write(json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n")
PY
chmod 600 "$attempt/out/receipt.json"

echo "computed-flow conformance: passed"
echo "attempt=$attempt"
echo "provider=$output"
echo "receipt=$attempt/out/receipt.json"
