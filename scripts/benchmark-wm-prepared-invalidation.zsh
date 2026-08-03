#!/bin/zsh

# Guarded warm-invalidation proof for activated prepared WM shards. Generated
# sources and raw Cargo output remain private beneath the explicit target.

set -eu
set -o pipefail
umask 077

typeset -r repo_root=${0:A:h:h}
typeset -r script_path=$0
typeset -r shards="$repo_root/examples/wm2000-block-shards"
typeset -r shard_manifest="$shards/Cargo.toml"
typeset -r audit="$repo_root/scripts/wm-prepared-audit.py"
typeset -r guard="$repo_root/scripts/memory-guard.zsh"
typeset mode=run
typeset prepared=
typeset target=

usage() {
    print -u2 -- "usage: $script_path [--dry-run] --prepared ABSOLUTE_PATH --target ABSOLUTE_PATH"
    print -u2 -- "       $script_path --selftest"
}

while (( $# > 0 )); do
    case $1 in
        --prepared)
            (( $# >= 2 )) || { usage; exit 2; }
            [[ -z $prepared ]] || { print -u2 -- "WM prepared invalidation: duplicate option"; exit 2; }
            prepared=$2
            shift 2
            ;;
        --target)
            (( $# >= 2 )) || { usage; exit 2; }
            [[ -z $target ]] || { print -u2 -- "WM prepared invalidation: duplicate option"; exit 2; }
            target=$2
            shift 2
            ;;
        --dry-run) mode=dry-run; shift ;;
        --selftest) mode=selftest; shift ;;
        -h|--help) usage; exit 0 ;;
        *) print -u2 -- "WM prepared invalidation: unknown option"; usage; exit 2 ;;
    esac
done

if [[ $mode == selftest ]]; then
    python3 "$audit" selftest --shards "$shards"
    exit $?
fi

[[ -n $prepared && -n $target ]] || { usage; exit 2; }
[[ $prepared == /* && $target == /* ]] || {
    print -u2 -- "WM prepared invalidation: private paths must be explicit absolute paths"
    exit 2
}

typeset -r activation=$(python3 "$audit" activation --shards "$shards")
if [[ $activation != active ]]; then
    print -r -- '{"schema":"fn64.wm-prepared-invalidation-benchmark.v1","status":"inactive","package_count":35,"shard_rustc_count":0}'
    exit 4
fi

if [[ $mode == dry-run ]]; then
    print -r -- '{"schema":"fn64.wm-prepared-invalidation-benchmark.v1","status":"dry-run","package_count":35,"cargo_jobs":1,"max_rss_mib":'${FN64_GUARD_MAX_RSS_MIB:-2048}',"min_free_percent":'${FN64_GUARD_MIN_FREE_PERCENT:-40}'}'
    exit 0
fi

python3 "$audit" validate-locations \
    --repo "$repo_root" \
    --outside "$prepared" --outside "$target" \
    --must-exist "$prepared" --must-be-absent "$target"

mkdir -m 700 "$target"
mkdir -m 700 "$target/raw" "$target/cargo-target"
typeset -r prepared_work="$target/prepared-work"
python3 "$audit" copy-tree --shards "$shards" --source "$prepared" --destination "$prepared_work"

export CARGO_BUILD_JOBS=1
export FN64_GUARD_MAX_RSS_MIB=${FN64_GUARD_MAX_RSS_MIB:-2048}
export FN64_GUARD_MIN_FREE_PERCENT=${FN64_GUARD_MIN_FREE_PERCENT:-40}
export FN64_GUARD_MAX_SECONDS=${FN64_GUARD_MAX_SECONDS:-2400}
export FN64_WM_PREPARED_SHARD_ROOT=$prepared_work
export CARGO_TARGET_DIR="$target/cargo-target"

run_phase() {
    local phase=$1
    local output="$target/raw/$phase.cargo.json"
    local samples="$target/raw/$phase.guard.jsonl"
    set +e
    FN64_GUARD_JSONL="$samples" \
        "$guard" /usr/bin/env -u ROM cargo build --locked -j1 --workspace \
            --manifest-path "$shard_manifest" --message-format=json-render-diagnostics \
            >"$output" 2>"$target/raw/$phase.stderr"
    local status=$?
    set -e
    if (( status != 0 )); then
        print -u2 -- "WM prepared invalidation: guarded build phase failed; private diagnostics retained"
        exit $status
    fi
}

run_phase cold
run_phase noop
python3 "$audit" mutate-root-claim --shards "$shards" --root "$prepared_work"
run_phase root-claim
python3 "$audit" mutate-one-artifact --shards "$shards" --root "$prepared_work"
run_phase one-artifact

set +e
FN64_GUARD_JSONL="$target/raw/metadata.guard.jsonl" \
    "$guard" /usr/bin/env -u ROM cargo metadata --locked --offline --format-version=1 \
        --manifest-path "$shard_manifest" \
        >"$target/raw/metadata.json" 2>"$target/raw/metadata.stderr"
typeset -ri metadata_status=$?
set -e
if (( metadata_status != 0 )); then
    print -u2 -- "WM prepared invalidation: guarded metadata graph failed; private diagnostics retained"
    exit $metadata_status
fi

python3 "$audit" compose-benchmark \
    --shards "$shards" \
    --cold-json "$target/raw/cold.cargo.json" --cold-guard "$target/raw/cold.guard.jsonl" \
    --noop-json "$target/raw/noop.cargo.json" --noop-guard "$target/raw/noop.guard.jsonl" \
    --root-json "$target/raw/root-claim.cargo.json" --root-guard "$target/raw/root-claim.guard.jsonl" \
    --artifact-json "$target/raw/one-artifact.cargo.json" --artifact-guard "$target/raw/one-artifact.guard.jsonl" \
    --metadata "$target/raw/metadata.json" \
    --prepared-work "$prepared_work"
