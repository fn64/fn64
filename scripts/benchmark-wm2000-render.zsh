#!/bin/zsh

# Run an already-built rs+wgpu shell through a bounded pump census and emit one
# swap-to-swap latency receipt per repetition. This script never rebuilds, so
# counterbalanced measurement controls can use the exact same binary.

set -eu
set -o pipefail

typeset -r script_path=$0
typeset -r fn64_root=$(cd -- "$(dirname -- "$script_path")/.." && pwd -P)
typeset binary_path="$fn64_root/crates/fn64-shell/rs/target/release/fn64"
typeset rom_path=${ROM:-}
typeset output_dir=
typeset label=run
typeset -i runs=1
typeset -i warmup=300
typeset -i pumps=800
typeset -i phase_profile=0

usage() {
    print -u2 -- "usage: $script_path --rom PATH [--bin PATH] [--output-dir PATH] [--label NAME]"
    print -u2 -- "       [--runs N] [--warmup N] [--pumps N] [--phase-profile] [-- ARGS...]"
}

positive_integer() {
    [[ $1 == <-> && $1 -gt 0 ]]
}

while (( $# > 0 )); do
    case $1 in
        --bin)
            (( $# >= 2 )) || { usage; exit 2; }
            binary_path=$2
            shift 2
            ;;
        --rom)
            (( $# >= 2 )) || { usage; exit 2; }
            rom_path=$2
            shift 2
            ;;
        --output-dir)
            (( $# >= 2 )) || { usage; exit 2; }
            output_dir=$2
            shift 2
            ;;
        --label)
            (( $# >= 2 )) || { usage; exit 2; }
            label=$2
            shift 2
            ;;
        --runs)
            (( $# >= 2 )) || { usage; exit 2; }
            positive_integer $2 || { print -u2 -- "benchmark-wm2000: --runs must be positive"; exit 2; }
            runs=$2
            shift 2
            ;;
        --warmup)
            (( $# >= 2 )) || { usage; exit 2; }
            positive_integer $2 || { print -u2 -- "benchmark-wm2000: --warmup must be positive"; exit 2; }
            warmup=$2
            shift 2
            ;;
        --pumps)
            (( $# >= 2 )) || { usage; exit 2; }
            positive_integer $2 || { print -u2 -- "benchmark-wm2000: --pumps must be positive"; exit 2; }
            pumps=$2
            shift 2
            ;;
        --phase-profile)
            phase_profile=1
            shift
            ;;
        --)
            shift
            break
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            print -u2 -- "benchmark-wm2000: unknown argument $1"
            usage
            exit 2
            ;;
    esac
done
typeset -a binary_args
binary_args=($@)

[[ -x $binary_path ]] || { print -u2 -- "benchmark-wm2000: binary is not executable: $binary_path"; exit 2; }
[[ -f $rom_path ]] || { print -u2 -- "benchmark-wm2000: --rom (or ROM) must name a readable ROM"; exit 2; }
(( pumps >= 4 )) || { print -u2 -- "benchmark-wm2000: --pumps must be at least 4"; exit 2; }
[[ -n $label && $label != *[^A-Za-z0-9._-]* ]] || {
    print -u2 -- "benchmark-wm2000: label must be path-safe"
    exit 2
}

typeset heavy_processes
heavy_processes=$(ps -Ao comm= | awk '$0 ~ /(^|\/)(cargo|rustc)$/ { count += 1 } END { print count + 0 }')
(( heavy_processes == 0 )) || {
    print -u2 -- "benchmark-wm2000: refusing a contended run: $heavy_processes cargo/rustc process(es) active"
    exit 1
}

if [[ -z $output_dir ]]; then
    output_dir=$(mktemp -d /private/tmp/fn64-wm2000-benchmark.XXXXXXXX)
else
    mkdir -p -- "$output_dir"
fi
chmod 700 "$output_dir"

typeset timing=0
typeset executor_split=0
typeset resume_split=0
typeset session_phase=0
if (( phase_profile )); then
    timing=1
    executor_split=1
    resume_split=1
    session_phase=1
fi

typeset mode=timing
(( phase_profile )) && mode=profile
print -- "benchmark-wm2000: output=$output_dir mode=$mode warmup=$warmup pumps=$pumps"
typeset -i run_index=1
while (( run_index <= runs )); do
    typeset run_name="${label}-$(printf '%02d' $run_index)"
    typeset log_path="$output_dir/$run_name.log"
    typeset summary_path="$output_dir/$run_name.json"
    print -- "benchmark-wm2000: starting $run_name"
    env \
        ROM="$rom_path" \
        FN64_ABSENT_N64DD=1 \
        FN64_RENDER=wgpu \
        FN64_PUMP_CENSUS=1 \
        FN64_PUMP_CENSUS_WARMUP="$warmup" \
        FN64_PUMP_CENSUS_PUMPS="$pumps" \
        FN64_PUMP_CENSUS_SEQUENCE="$pumps" \
        FN64_PHASE_TIMING="$timing" \
        FN64_EXECUTOR_SPLIT="$executor_split" \
        FN64_RESUME_SPLIT="$resume_split" \
        FN64_SESSION_PHASE_CENSUS="$session_phase" \
        FN64_DPC_COPY_CENSUS="$phase_profile" \
        "$binary_path" "${binary_args[@]}" >"$log_path" 2>&1
    python3 "$fn64_root/tools/summarize_wm2000_pump_census.py" \
        "$log_path" --output "$summary_path"
    python3 -c \
        'import json,sys; r=json.load(open(sys.argv[1])); d=r["drawn_frame_ms"]; print(f"benchmark-wm2000: {sys.argv[2]} mean={d['"'"'mean'"'"']:.3f} p95={d['"'"'p95'"'"']:.3f} p99={d['"'"'p99'"'"']:.3f} max={d['"'"'max'"'"']:.3f} ms gap2={100*r['"'"'gap_two_fraction'"'"']:.1f}% over={100*r['"'"'over_budget'"'"']['"'"'fraction'"'"']:.1f}%")' \
        "$summary_path" "$run_name"
    (( run_index += 1 ))
done
