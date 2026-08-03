#!/bin/sh
set -eu
umask 077

repo=$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd)
fixture=$(mktemp -d "${TMPDIR:-/tmp}/fn64-n64loaderwv-series-test.XXXXXXXX")
trap 'rm -rf -- "$fixture"' EXIT HUP INT TERM
fixture_repo="$fixture/repo"
mkdir -p "$fixture_repo/tools/ghidra"
cp "$repo/tools/ghidra/run-n64loaderwv-conformance-series.sh" \
    "$fixture_repo/tools/ghidra/run-n64loaderwv-conformance-series.sh"

cat > "$fixture_repo/tools/ghidra/run-n64loaderwv-conformance.sh" <<'EOF'
#!/bin/sh
set -eu
run_name=$(basename "$FN64_GHIDRA_WORK")
[ "${FN64_SYNTH_FAIL_RUN:-}" != "$run_name" ] || exit 9
attempt="$FN64_GHIDRA_WORK/attempt.synthetic"
mkdir -p "$attempt/out"
extension_sha=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
if [ "${FN64_SYNTH_DIFFERENT_EXTENSION_RUN:-}" = "$run_name" ]; then
    extension_sha=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
fi
printf '%s\n' 'schema=fn64.n64loaderwv-conformance.v2' > "$attempt/out/receipt.txt"
printf 'n64loaderwv_extension_sha256=%s\n' "$extension_sha" >> "$attempt/out/receipt.txt"
printf 'run=%s\n' "$run_name" >> "$attempt/out/receipt.txt"
EOF
chmod 700 "$fixture_repo/tools/ghidra/run-n64loaderwv-conformance.sh" \
    "$fixture_repo/tools/ghidra/run-n64loaderwv-conformance-series.sh"

series="$fixture/series"
mkdir -m 700 "$series"
FN64_GHIDRA_SERIES_WORK="$series" \
    "$fixture_repo/tools/ghidra/run-n64loaderwv-conformance-series.sh" 3 >/dev/null
[ "$(sed -n '1p' "$series/receipt.txt")" = \
    'schema=fn64.n64loaderwv-conformance-series.v1' ]
[ "$(sed -n '2p' "$series/receipt.txt")" = 'run_count=3' ]
[ "$(sed -n '3p' "$series/receipt.txt")" = \
    'n64loaderwv_extension_sha256=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' ]
[ "$(wc -l < "$series/attempt-receipts.txt" | tr -d ' ')" -eq 3 ]
if grep -q "$fixture" "$series/receipt.txt" "$series/attempt-receipts.txt"; then
    echo "n64loaderwv conformance series test: receipt disclosed a path" >&2
    exit 1
fi

too_many="$fixture/too-many"
mkdir -m 700 "$too_many"
if FN64_GHIDRA_SERIES_WORK="$too_many" \
        "$fixture_repo/tools/ghidra/run-n64loaderwv-conformance-series.sh" 101 >/dev/null 2>&1; then
    echo "n64loaderwv conformance series test: accepted excessive run count" >&2
    exit 1
fi

fail_fast="$fixture/fail-fast"
mkdir -m 700 "$fail_fast"
if FN64_SYNTH_FAIL_RUN=run-002 FN64_GHIDRA_SERIES_WORK="$fail_fast" \
        "$fixture_repo/tools/ghidra/run-n64loaderwv-conformance-series.sh" 3 >/dev/null 2>&1; then
    echo "n64loaderwv conformance series test: accepted a failed attempt" >&2
    exit 1
fi
[ ! -e "$fail_fast/run-003" ] || {
    echo "n64loaderwv conformance series test: did not fail fast" >&2
    exit 1
}
[ ! -e "$fail_fast/receipt.txt" ] || {
    echo "n64loaderwv conformance series test: published receipt after failure" >&2
    exit 1
}

non_reproducible="$fixture/non-reproducible"
mkdir -m 700 "$non_reproducible"
if FN64_SYNTH_DIFFERENT_EXTENSION_RUN=run-002 \
        FN64_GHIDRA_SERIES_WORK="$non_reproducible" \
        "$fixture_repo/tools/ghidra/run-n64loaderwv-conformance-series.sh" 3 >/dev/null 2>&1; then
    echo "n64loaderwv conformance series test: accepted extension digest drift" >&2
    exit 1
fi
[ ! -e "$non_reproducible/run-003" ] || {
    echo "n64loaderwv conformance series test: did not fail fast on extension drift" >&2
    exit 1
}
[ ! -e "$non_reproducible/receipt.txt" ] || {
    echo "n64loaderwv conformance series test: published receipt after extension drift" >&2
    exit 1
}

echo "n64loaderwv conformance series test: PASS"
