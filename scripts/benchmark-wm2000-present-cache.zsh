#!/bin/zsh

# Run counterbalanced Observe/Suppress presentation-cache pairs with one exact
# binary, then require canonical per-pump dependency parity for every pair.

set -eu
set -o pipefail

typeset -r script_path=$0
typeset -r fn64_root=$(cd -- "$(dirname -- "$script_path")/.." && pwd -P)
typeset -r runner="$fn64_root/scripts/benchmark-wm2000-render.zsh"
typeset -r comparator="$fn64_root/tools/compare_wm2000_present_dependencies.py"
typeset binary_path="$fn64_root/crates/fn64-shell/rs/target/release/fn64"
typeset rom_path=${ROM:-}
typeset output_dir=
typeset -i pairs=1
typeset -i warmup=300
typeset -i pumps=1600

usage() {
    print -u2 -- "usage: $script_path --rom PATH [--bin PATH] --output-dir PATH"
    print -u2 -- "       [--pairs N] [--warmup N] [--pumps 1600]"
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
        --pairs)
            (( $# >= 2 )) || { usage; exit 2; }
            positive_integer $2 || { print -u2 -- "benchmark-wm2000-present-cache: --pairs must be positive"; exit 2; }
            pairs=$2
            shift 2
            ;;
        --warmup)
            (( $# >= 2 )) || { usage; exit 2; }
            positive_integer $2 || { print -u2 -- "benchmark-wm2000-present-cache: --warmup must be positive"; exit 2; }
            warmup=$2
            shift 2
            ;;
        --pumps)
            (( $# >= 2 )) || { usage; exit 2; }
            positive_integer $2 || { print -u2 -- "benchmark-wm2000-present-cache: --pumps must be positive"; exit 2; }
            pumps=$2
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            print -u2 -- "benchmark-wm2000-present-cache: unknown argument $1"
            usage
            exit 2
            ;;
    esac
done

[[ -x $binary_path ]] || { print -u2 -- "benchmark-wm2000-present-cache: binary is not executable: $binary_path"; exit 2; }
[[ -f $rom_path ]] || { print -u2 -- "benchmark-wm2000-present-cache: --rom must name a readable ROM"; exit 2; }
[[ -n $output_dir ]] || { print -u2 -- "benchmark-wm2000-present-cache: --output-dir is required"; exit 2; }
(( pumps == 1600 )) || {
    print -u2 -- "benchmark-wm2000-present-cache: parity authority requires exactly 1600 measured pumps"
    exit 2
}

mkdir -p -- "$output_dir"
chmod 700 "$output_dir"

typeset -i pair=1
while (( pair <= pairs )); do
    typeset pair_name="pair-$(printf '%02d' $pair)"
    typeset pair_dir="$output_dir/$pair_name"
    mkdir -p -- "$pair_dir"
    typeset -a modes
    if (( pair % 2 == 1 )); then
        modes=(observe 1)
    else
        modes=(1 observe)
    fi
    for mode in $modes; do
        typeset label=observe
        [[ $mode == 1 ]] && label=suppress
        FN64_PRESENT_CACHE=$mode "$runner" \
            --rom "$rom_path" \
            --bin "$binary_path" \
            --output-dir "$pair_dir" \
            --label "$label" \
            --runs 1 \
            --warmup "$warmup" \
            --pumps "$pumps"
    done
    python3 "$comparator" \
        "$pair_dir/observe-01.log" \
        "$pair_dir/suppress-01.log" \
        --output "$pair_dir/present-parity.json"
    print -- "benchmark-wm2000-present-cache: $pair_name canonical parity PASS"
    (( pair += 1 ))
done
