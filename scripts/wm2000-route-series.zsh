#!/bin/zsh

# Repeat one guarded WM2000 route sequentially and require byte-identical
# guest evidence. Full stdout logs remain in a fresh /tmp directory.

set -u

if (( $# < 3 || $# > 4 )); then
    print -u2 -- "usage: ROM=... FN64_BOOT_CONTEXT=... $0 SCHEDULE MAX_STEPS RUNS [STOP_GENERATION]"
    exit 2
fi

typeset -r series_root=${0:A:h:h}
typeset -r series_schedule=${1:A}
typeset -r series_max_steps=$2
typeset -r series_runs=$3
typeset -r series_logs=$(mktemp -d /private/tmp/fn64-wm-route-series.XXXXXX)
typeset -r series_baseline=$series_logs/baseline.evidence

if [[ ! "$series_runs" =~ '^[0-9]+$' || "$series_runs" == 0 ]]; then
    print -u2 -- "wm2000 route series: RUNS must be a positive integer"
    exit 2
fi

for run_index in {1..$series_runs}; do
    typeset run_log=$series_logs/run-$run_index.log
    typeset run_evidence=$series_logs/run-$run_index.evidence
    print -u2 -- "wm2000 route series: run $run_index/$series_runs"
    typeset -a probe_arguments
    probe_arguments=("$series_schedule" "$series_max_steps")
    if (( $# == 4 )); then
        probe_arguments+=("$4")
    fi
    if ! "$series_root/scripts/wm2000-route-probe.zsh" "${probe_arguments[@]}" >"$run_log" 2>&1; then
        print -u2 -- "wm2000 route series: run $run_index failed; logs retained at $series_logs"
        exit 1
    fi
    awk '
        /controller schedule=/ ||
        /controller input_edge/ ||
        /first generation=/ ||
        /stop generation/ ||
        /\[wm2000-block-boot\] done:/ ||
        /\[wm2000-block-progress\]/ ||
        /entered digest-selected ROM-recovered generations/
    ' "$run_log" >"$run_evidence"
    if (( run_index == 1 )); then
        cp "$run_evidence" "$series_baseline"
    elif ! cmp -s "$series_baseline" "$run_evidence"; then
        print -u2 -- "wm2000 route series: run $run_index differs; logs retained at $series_logs"
        diff -u "$series_baseline" "$run_evidence" || true
        exit 1
    fi
    shasum -a 256 "$run_evidence"
done

print -- "wm2000 route series: $series_runs/$series_runs evidence-identical; logs=$series_logs"
