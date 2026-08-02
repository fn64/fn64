#!/bin/sh
set -eu
umask 077

repo=$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd)
runner="$repo/tools/ghidra/run-conformance.sh"
run_count=${1:-}
series_work=${FN64_GHIDRA_SERIES_WORK:-}

fail() {
    echo "ghidra conformance series: $1" >&2
    exit 2
}

if [ "$#" -ne 1 ]; then
    fail "usage: $0 RUN_COUNT"
fi
case "$run_count" in
    ''|0|*[!0-9]*) fail "RUN_COUNT must be a positive integer" ;;
esac
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
[ -x "$runner" ] || fail "run-conformance.sh is not executable"

require_sha() {
    case "$1" in
        ''|*[!0-9a-f]*) return 1 ;;
    esac
    [ "${#1}" -eq 64 ]
}

baseline_a_seeded=
baseline_a_unseeded=
baseline_b_unseeded=
run_index=1
while [ "$run_index" -le "$run_count" ]; do
    attempt=$(mktemp -d "$series_work/attempt-$run_index.XXXXXXXX") ||
        fail "could not create attempt $run_index"
    chmod 700 "$attempt"
    run_log="$attempt/run-conformance.log"
    echo "ghidra conformance series: run $run_index/$run_count; evidence=$attempt" >&2
    if ! FN64_GHIDRA_WORK="$attempt" "$runner" >"$run_log" 2>&1; then
        fail "run $run_index/$run_count failed; evidence retained at $attempt"
    fi

    digest_line=$(sed -n \
        's/^one-run digests (ten-run refresh pending): //p' "$run_log")
    # The runner owns this fixed, three-field machine-readable suffix.
    digest_line_count=$(printf '%s\n' "$digest_line" |
        awk 'NF { count++ } END { print count + 0 }')
    [ "$digest_line_count" -eq 1 ] ||
        fail "run $run_index/$run_count did not report exactly three digests; evidence retained at $attempt"
    IFS=' ' read -r seeded_field unseeded_a_field unseeded_b_field extra_fields <<EOF
$digest_line
EOF
    [ -z "$extra_fields" ] ||
        fail "run $run_index/$run_count did not report exactly three digests; evidence retained at $attempt"
    case "$seeded_field" in bank-a-seeded=*) a_seeded=${seeded_field#bank-a-seeded=} ;; *) fail "run $run_index reported an invalid seeded-A digest field" ;; esac
    case "$unseeded_a_field" in bank-a-unseeded=*) a_unseeded=${unseeded_a_field#bank-a-unseeded=} ;; *) fail "run $run_index reported an invalid unseeded-A digest field" ;; esac
    case "$unseeded_b_field" in bank-b-unseeded=*) b_unseeded=${unseeded_b_field#bank-b-unseeded=} ;; *) fail "run $run_index reported an invalid unseeded-B digest field" ;; esac
    require_sha "$a_seeded" || fail "run $run_index reported an invalid seeded-A digest"
    require_sha "$a_unseeded" || fail "run $run_index reported an invalid unseeded-A digest"
    require_sha "$b_unseeded" || fail "run $run_index reported an invalid unseeded-B digest"
    printf '%s\n' \
        "bank-a-seeded=$a_seeded" \
        "bank-a-unseeded=$a_unseeded" \
        "bank-b-unseeded=$b_unseeded" >"$attempt/digests.txt"

    if [ "$run_index" -eq 1 ]; then
        baseline_a_seeded=$a_seeded
        baseline_a_unseeded=$a_unseeded
        baseline_b_unseeded=$b_unseeded
    elif [ "$a_seeded" != "$baseline_a_seeded" ] ||
            [ "$a_unseeded" != "$baseline_a_unseeded" ] ||
            [ "$b_unseeded" != "$baseline_b_unseeded" ]; then
        echo "ghidra conformance series: baseline bank-a-seeded=$baseline_a_seeded bank-a-unseeded=$baseline_a_unseeded bank-b-unseeded=$baseline_b_unseeded" >&2
        echo "ghidra conformance series: run-$run_index bank-a-seeded=$a_seeded bank-a-unseeded=$a_unseeded bank-b-unseeded=$b_unseeded" >&2
        fail "run $run_index/$run_count digest mismatch; evidence retained at $attempt"
    fi

    run_index=$((run_index + 1))
done

echo "ghidra conformance series: $run_count/$run_count digest-identical clean runs"
echo "stable digests: bank-a-seeded=$baseline_a_seeded bank-a-unseeded=$baseline_a_unseeded bank-b-unseeded=$baseline_b_unseeded"
echo "evidence retained at $series_work"
