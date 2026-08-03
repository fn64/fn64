#!/bin/zsh

# Ten fresh, content-silent comparisons of the shared WM producer against the
# legacy one-package Cargo build. Private raw output remains under --target.

set -eu
set -o pipefail
umask 077

typeset -r repo_root=${0:A:h:h}
typeset -r script_path=$0
typeset -r shards="$repo_root/examples/wm2000-block-shards"
typeset -r shard_manifest="$shards/Cargo.toml"
typeset -r producer_manifest="$repo_root/examples/wm2000-prepared-shard-producer/Cargo.toml"
typeset -r audit="$repo_root/scripts/wm-prepared-audit.py"
typeset -r guard="$repo_root/scripts/memory-guard.zsh"
typeset mode=run
typeset rom=
typeset prepared_parent=
typeset target=
typeset generator_digest=
typeset discovery_digest=
typeset emitter_digest=
typeset runtime_digest=

usage() {
    print -u2 -- "usage: $script_path [--dry-run] --rom ABSOLUTE_PATH --prepared-parent ABSOLUTE_PATH --target ABSOLUTE_PATH --generator-source-sha256 DIGEST --discovery-source-sha256 DIGEST --emitter-source-sha256 DIGEST --runtime-source-sha256 DIGEST"
    print -u2 -- "       $script_path --selftest"
}

set_once() {
    local name=$1
    local value=$2
    [[ -z ${(P)name} ]] || {
        print -u2 -- "WM prepared parity: duplicate option"
        exit 2
    }
    typeset -g "$name=$value"
}

while (( $# > 0 )); do
    case $1 in
        --rom|--prepared-parent|--target|--generator-source-sha256|--discovery-source-sha256|--emitter-source-sha256|--runtime-source-sha256)
            (( $# >= 2 )) || { usage; exit 2; }
            case $1 in
                --rom) set_once rom "$2" ;;
                --prepared-parent) set_once prepared_parent "$2" ;;
                --target) set_once target "$2" ;;
                --generator-source-sha256) set_once generator_digest "$2" ;;
                --discovery-source-sha256) set_once discovery_digest "$2" ;;
                --emitter-source-sha256) set_once emitter_digest "$2" ;;
                --runtime-source-sha256) set_once runtime_digest "$2" ;;
            esac
            shift 2
            ;;
        --dry-run) mode=dry-run; shift ;;
        --selftest) mode=selftest; shift ;;
        -h|--help) usage; exit 0 ;;
        *) print -u2 -- "WM prepared parity: unknown option"; usage; exit 2 ;;
    esac
done

if [[ $mode == selftest ]]; then
    python3 "$audit" selftest --shards "$shards"
    exit $?
fi

for required in rom prepared_parent target generator_digest discovery_digest emitter_digest runtime_digest; do
    [[ -n ${(P)required} ]] || { usage; exit 2; }
done
for digest in "$generator_digest" "$discovery_digest" "$emitter_digest" "$runtime_digest"; do
    [[ ${#digest} == 64 && $digest != *[^0-9a-f]* && $digest != 0000000000000000000000000000000000000000000000000000000000000000 ]] || {
        print -u2 -- "WM prepared parity: source identities must be nonzero lowercase SHA-256"
        exit 2
    }
done
for private_path in "$rom" "$prepared_parent" "$target"; do
    [[ $private_path == /* ]] || {
        print -u2 -- "WM prepared parity: private paths must be explicit absolute paths"
        exit 2
    }
done

if [[ $mode == dry-run ]]; then
    print -r -- '{"schema":"fn64.wm-prepared-parity-audit.v1","status":"dry-run","publication_count":10,"package_count":35,"cargo_jobs":1,"max_rss_mib":'${FN64_GUARD_MAX_RSS_MIB:-2048}',"min_free_percent":'${FN64_GUARD_MIN_FREE_PERCENT:-40}'}'
    exit 0
fi

python3 "$audit" validate-locations \
    --repo "$repo_root" \
    --outside "$rom" --outside "$prepared_parent" --outside "$target" \
    --must-exist "$rom" --must-exist "$prepared_parent" \
    --must-be-absent "$target"

typeset publication
for publication in {00..09}; do
    python3 "$audit" validate-locations \
        --repo "$repo_root" \
        --outside "$prepared_parent/publication-$publication" \
        --must-be-absent "$prepared_parent/publication-$publication"
done

mkdir -m 700 "$target"
mkdir -m 700 "$target/raw" "$target/producer-target" "$target/legacy-target"
typeset -r staged_rom="$target/private-input.rom"
python3 "$audit" stage-private-file --source "$rom" --destination "$staged_rom"

export CARGO_BUILD_JOBS=1
export FN64_GUARD_MAX_RSS_MIB=${FN64_GUARD_MAX_RSS_MIB:-2048}
export FN64_GUARD_MIN_FREE_PERCENT=${FN64_GUARD_MIN_FREE_PERCENT:-40}
export FN64_GUARD_MAX_SECONDS=${FN64_GUARD_MAX_SECONDS:-2400}

typeset -r producer_json="$target/raw/producer-cargo.json"
typeset -r producer_guard="$target/raw/producer-cargo.guard.jsonl"
set +e
CARGO_TARGET_DIR="$target/producer-target" FN64_GUARD_JSONL="$producer_guard" \
    "$guard" cargo build --locked -j1 --manifest-path "$producer_manifest" \
        --message-format=json-render-diagnostics >"$producer_json" 2>"$target/raw/producer-cargo.stderr"
typeset -ri producer_status=$?
set -e
if (( producer_status != 0 )); then
    print -u2 -- "WM prepared parity: guarded producer compilation failed; private diagnostics retained"
    exit $producer_status
fi

typeset -a publication_guards
for publication in {00..09}; do
    typeset publication_guard="$target/raw/publication-$publication.guard.jsonl"
    publication_guards+=("$publication_guard")
    set +e
    FN64_GUARD_JSONL="$publication_guard" \
        "$guard" "$target/producer-target/debug/fn64-wm-prepared-shard-producer" \
            --rom "$staged_rom" \
            --output "$prepared_parent/publication-$publication" \
            --generator-source-sha256 "$generator_digest" \
            --discovery-source-sha256 "$discovery_digest" \
            --emitter-source-sha256 "$emitter_digest" \
            --runtime-source-sha256 "$runtime_digest" \
            >"$target/raw/publication-$publication.stdout" \
            2>"$target/raw/publication-$publication.stderr"
    typeset -ri publication_status=$?
    set -e
    if (( publication_status != 0 )); then
        print -u2 -- "WM prepared parity: guarded fresh publication failed; private diagnostics retained"
        exit $publication_status
    fi
done

typeset -r legacy_json="$target/raw/legacy-cargo.json"
typeset -r legacy_guard="$target/raw/legacy-cargo.guard.jsonl"
set +e
ROM="$staged_rom" CARGO_TARGET_DIR="$target/legacy-target" FN64_GUARD_JSONL="$legacy_guard" \
    "$guard" cargo build --locked -j1 --workspace --manifest-path "$shard_manifest" \
        --message-format=json-render-diagnostics >"$legacy_json" 2>"$target/raw/legacy-cargo.stderr"
typeset -ri legacy_status=$?
set -e
if (( legacy_status != 0 )); then
    print -u2 -- "WM prepared parity: guarded legacy build failed; private diagnostics retained"
    exit $legacy_status
fi

typeset -a compose=(
    compose-parity
    --shards "$shards"
    --legacy-target "$target/legacy-target"
    --prepared-parent "$prepared_parent"
    --runs 10
    --producer-json "$producer_json"
    --producer-guard "$producer_guard"
    --legacy-json "$legacy_json"
    --legacy-guard "$legacy_guard"
)
for publication_guard in "${publication_guards[@]}"; do
    compose+=(--publication-guard "$publication_guard")
done
python3 "$audit" "${compose[@]}"
