#!/bin/sh
set -eu
umask 077

repo=$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd)
runner="$repo/tools/ghidra/run-n64loaderwv-conformance.sh"
run_count=${1:-}
series_work=${FN64_GHIDRA_SERIES_WORK:-}
max_runs=100

fail() {
    echo "n64loaderwv conformance series: $1" >&2
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

if [ "$#" -ne 1 ]; then
    fail "usage: $0 RUN_COUNT"
fi
case "$run_count" in
    ''|0|*[!0-9]*) fail "RUN_COUNT must be a positive integer" ;;
esac
[ "$run_count" -le "$max_runs" ] || fail "RUN_COUNT must not exceed $max_runs"
[ -n "$series_work" ] || fail "FN64_GHIDRA_SERIES_WORK is required"
case "$series_work" in
    /*) ;;
    *) fail "FN64_GHIDRA_SERIES_WORK must be absolute" ;;
esac
[ -d "$series_work" ] || fail "FN64_GHIDRA_SERIES_WORK must be a pre-existing directory"
[ ! -L "$series_work" ] || fail "FN64_GHIDRA_SERIES_WORK must not be a symlink"
series_work=$(CDPATH='' cd -- "$series_work" && pwd -P) ||
    fail "could not resolve FN64_GHIDRA_SERIES_WORK"
case "$series_work" in
    "$repo"|"$repo"/*) fail "FN64_GHIDRA_SERIES_WORK must be outside the repository" ;;
esac
if series_mode=$(stat -c '%a' "$series_work" 2>/dev/null); then
    :
elif series_mode=$(stat -f '%Lp' "$series_work" 2>/dev/null); then
    :
else
    fail "could not inspect FN64_GHIDRA_SERIES_WORK permissions"
fi
[ "$series_mode" = 700 ] || fail "FN64_GHIDRA_SERIES_WORK must have mode 0700"
[ -x "$runner" ] || fail "run-n64loaderwv-conformance.sh is not executable"
[ ! -e "$series_work/receipt.txt" ] || fail "series receipt already exists"
[ ! -e "$series_work/attempt-receipts.txt" ] || fail "attempt receipt list already exists"

receipt_list="$series_work/attempt-receipts.txt"
: > "$receipt_list"
common_extension_sha=
run_index=1
while [ "$run_index" -le "$run_count" ]; do
    run_work="$series_work/run-$(printf '%03d' "$run_index")"
    [ ! -e "$run_work" ] || fail "run $run_index workspace already exists"
    mkdir -m 700 "$run_work"
    run_log="$run_work/run.log"
    echo "n64loaderwv conformance series: run $run_index/$run_count" >&2
    if ! FN64_GHIDRA_WORK="$run_work" "$runner" > "$run_log" 2>&1; then
        fail "run $run_index/$run_count failed"
    fi

    receipt=
    receipt_count=0
    for candidate in "$run_work"/attempt.*/out/receipt.txt; do
        [ -f "$candidate" ] || continue
        receipt=$candidate
        receipt_count=$((receipt_count + 1))
    done
    [ "$receipt_count" -eq 1 ] ||
        fail "run $run_index/$run_count did not produce exactly one receipt"
    [ "$(sed -n '1p' "$receipt")" = 'schema=fn64.n64loaderwv-conformance.v2' ] ||
        fail "run $run_index/$run_count produced an unexpected receipt schema"
    extension_sha=$(sed -n 's/^n64loaderwv_extension_sha256=//p' "$receipt")
    case "$extension_sha" in
        *[!0-9a-f]*|'') fail "run $run_index/$run_count produced an invalid extension digest" ;;
    esac
    [ "${#extension_sha}" -eq 64 ] ||
        fail "run $run_index/$run_count produced an invalid extension digest"
    [ "$(grep -c '^n64loaderwv_extension_sha256=' "$receipt")" -eq 1 ] ||
        fail "run $run_index/$run_count did not bind exactly one extension digest"
    if [ -z "$common_extension_sha" ]; then
        common_extension_sha=$extension_sha
    else
        [ "$extension_sha" = "$common_extension_sha" ] ||
            fail "run $run_index/$run_count produced a non-reproducible extension archive"
    fi
    receipt_sha=$(hash_file "$receipt")
    printf 'attempt_%03d_receipt_sha256=%s\n' "$run_index" "$receipt_sha" >> "$receipt_list"
    run_index=$((run_index + 1))
done

receipt_list_sha=$(hash_file "$receipt_list")
series_receipt="$series_work/receipt.txt"
{
    printf '%s\n' 'schema=fn64.n64loaderwv-conformance-series.v1'
    printf 'run_count=%s\n' "$run_count"
    printf 'n64loaderwv_extension_sha256=%s\n' "$common_extension_sha"
    printf 'attempt_receipts_sha256=%s\n' "$receipt_list_sha"
    cat "$receipt_list"
} > "$series_receipt"
series_receipt_sha=$(hash_file "$series_receipt")

echo "n64loaderwv conformance series: $run_count/$run_count clean runs receipt_sha256=$series_receipt_sha"
