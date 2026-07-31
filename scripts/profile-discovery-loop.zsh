#!/bin/zsh

# Repeat the compact fn64-discover feedback path without retaining ROM-derived
# output.  The command emits only a path-free timing receipt; the individual
# discovery summaries stay in a mode-0700 temporary directory until exit.

set -eu
set -o pipefail

typeset -r script_path=$0
typeset discovery_bin=${FN64_DISCOVER_BIN:-}
typeset rom_path=
typeset evidence_path=
typeset -a trace_paths
typeset -i runs=3
typeset -i max_seconds=120

usage() {
    print -u2 -- "usage: $script_path --bin PATH --rom PATH [--evidence PATH] [--trace PATH]... [--runs N] [--max-seconds N]"
    print -u2 -- "       FN64_DISCOVER_BIN=PATH $script_path --rom PATH ..."
}

positive_integer() {
    [[ $1 == <-> && $1 -gt 0 ]]
}

while (( $# > 0 )); do
    case $1 in
        --bin)
            (( $# >= 2 )) || { usage; exit 2; }
            discovery_bin=$2
            shift 2
            ;;
        --rom)
            (( $# >= 2 )) || { usage; exit 2; }
            rom_path=$2
            shift 2
            ;;
        --evidence)
            (( $# >= 2 )) || { usage; exit 2; }
            evidence_path=$2
            shift 2
            ;;
        --trace)
            (( $# >= 2 )) || { usage; exit 2; }
            trace_paths+=($2)
            shift 2
            ;;
        --runs)
            (( $# >= 2 )) || { usage; exit 2; }
            positive_integer $2 || { print -u2 -- "profile discovery loop: --runs must be a positive integer"; exit 2; }
            runs=$2
            shift 2
            ;;
        --max-seconds)
            (( $# >= 2 )) || { usage; exit 2; }
            positive_integer $2 || { print -u2 -- "profile discovery loop: --max-seconds must be a positive integer"; exit 2; }
            max_seconds=$2
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            print -u2 -- "profile discovery loop: unknown argument $1"
            usage
            exit 2
            ;;
    esac
done

[[ -n $discovery_bin && -x $discovery_bin ]] || {
    print -u2 -- "profile discovery loop: --bin (or FN64_DISCOVER_BIN) must name an executable fn64-discover binary"
    exit 2
}
[[ -n $rom_path && -f $rom_path ]] || {
    print -u2 -- "profile discovery loop: --rom must name a readable local ROM"
    exit 2
}
[[ -z $evidence_path || -f $evidence_path ]] || {
    print -u2 -- "profile discovery loop: --evidence must name a readable file"
    exit 2
}
for trace_path in $trace_paths; do
    [[ -f $trace_path ]] || {
        print -u2 -- "profile discovery loop: every --trace path must be readable"
        exit 2
    }
done

zmodload zsh/datetime
typeset work
work=$(mktemp -d "${TMPDIR:-/tmp}/fn64-discovery-profile.XXXXXXXX")
chmod 700 "$work"
cleanup() { rm -rf -- "$work"; }
trap cleanup EXIT HUP INT TERM

typeset -a milliseconds
typeset baseline=
typeset -i run
for (( run = 1; run <= runs; run++ )); do
    typeset output="$work/$run.json"
    typeset diagnostics="$work/$run.stderr"
    typeset start=$EPOCHREALTIME
    typeset -a command
    command=("$discovery_bin" "$rom_path")
    if [[ -n $evidence_path ]]; then
        command+=(--evidence "$evidence_path")
    fi
    for trace_path in $trace_paths; do
        command+=(--trace "$trace_path")
    done
    command+=(--summary)
    "${command[@]}" >"$output" 2>"$diagnostics" &
    typeset child=$!
    (
        sleep "$max_seconds"
        if kill -0 "$child" 2>/dev/null; then
            kill -TERM "$child" 2>/dev/null || true
        fi
    ) &
    typeset watchdog=$!
    set +e
    wait "$child"
    typeset exit_status=$?
    set -e
    kill "$watchdog" 2>/dev/null || true
    wait "$watchdog" 2>/dev/null || true
    (( exit_status == 0 )) || {
        print -u2 -- "profile discovery loop: discovery failed or exceeded the ${max_seconds}s bound; private diagnostics retained only until exit"
        exit "$exit_status"
    }
    typeset elapsed_ms=$(( (EPOCHREALTIME - start) * 1000.0 ))
    milliseconds+=($elapsed_ms)
    typeset receipt
    receipt=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["receipt_sha256"])' "$output")
    if [[ -z $baseline ]]; then
        baseline=$receipt
    elif [[ $receipt != $baseline ]]; then
        print -u2 -- "profile discovery loop: summary receipt changed between runs"
        exit 1
    fi
done

typeset sorted
sorted=(${(on)milliseconds})
typeset -i count=${#milliseconds}
typeset -i median_index=$(( (count + 1) / 2 ))
python3 - "$baseline" "$count" "${sorted[1]}" "${sorted[median_index]}" "${sorted[-1]}" "$max_seconds" <<'PY'
import json
import sys
receipt, runs, minimum, median, maximum, bound = sys.argv[1:]
print(json.dumps({
    "schema": "fn64.discovery-summary-profile.v1",
    "status": "pass",
    "runs": int(runs),
    "min_ms": round(float(minimum)),
    "median_ms": round(float(median)),
    "max_ms": round(float(maximum)),
    "max_seconds": int(bound),
    "summary_receipt_sha256": receipt,
}, separators=(",", ":")))
PY
