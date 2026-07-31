#!/bin/zsh

# One cold, isolated, memory-guarded build of one generated WM2000 AOT shard.
# The retained target is intentionally never removed by this script.

set -eu
set -o pipefail

typeset -r repo_root=${0:A:h:h}
typeset -r script_path=$0
typeset -r shard_manifest="$repo_root/examples/wm2000-block-shards/Cargo.toml"
typeset -r default_package=wm2000-block-overlay-2-shard-04
typeset package=$default_package
typeset mode=run

usage() {
    print -u2 -- "usage: $script_path [--package PACKAGE]"
    print -u2 -- "       $script_path --dry-run [--package PACKAGE]"
    print -u2 -- "       $script_path --selftest"
}

package_exists() {
    local candidate=$1
    local manifest
    for manifest in "$repo_root"/examples/wm2000-block-shards/*/Cargo.toml(N); do
        if grep -Fqx "name = \"$candidate\"" "$manifest"; then
            return 0
        fi
    done
    return 1
}

reuse_inventory_fields() {
    print -r -- "$1" | sed -nE \
        's/.*reuse_2k_total_slots=([0-9]+) reuse_2k_unique_slots=([0-9]+) reuse_64k_total_slots=([0-9]+) reuse_64k_unique_slots=([0-9]+).*/\1 \2 \3 \4/p'
}

selftest() {
    package_exists "$default_package" || {
        print -u2 -- "profile wm2000 shard self-test: default package is absent"
        return 1
    }
    [[ -x "$repo_root/scripts/memory-guard.zsh" ]] || {
        print -u2 -- "profile wm2000 shard self-test: memory guard is not executable"
        return 1
    }
    [[ -f "$shard_manifest" ]] || {
        print -u2 -- "profile wm2000 shard self-test: shard manifest is absent"
        return 1
    }
    typeset sample='/private/input/owned game.z64'
    typeset sanitized
    sanitized=$(ROM="$sample" python3 -c \
        'import os, sys; sys.stdout.write(sys.stdin.read().replace(os.environ["ROM"], "<ROM>"))' \
        <<<"failure reading $sample")
    [[ "$sanitized" == 'failure reading <ROM>' ]] || {
        print -u2 -- "profile wm2000 shard self-test: ROM-path sanitizer failed"
        return 1
    }
    [[ "$(reuse_inventory_fields 'build-profile total_ms=1 reuse_2k_total_slots=12 reuse_2k_unique_slots=10 reuse_64k_total_slots=12 reuse_64k_unique_slots=8')" == '12 10 12 8' ]] || {
        print -u2 -- "profile wm2000 shard self-test: reuse inventory parser failed"
        return 1
    }
    print -- "profile wm2000 shard self-test: PASS"
}

while (( $# > 0 )); do
    case $1 in
        --package)
            (( $# >= 2 )) || {
                usage
                exit 2
            }
            package=$2
            shift 2
            ;;
        --dry-run)
            mode=dry-run
            shift
            ;;
        --selftest)
            mode=selftest
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            print -u2 -- "profile wm2000 shard: unknown argument $1"
            usage
            exit 2
            ;;
    esac
done

if [[ "$mode" == selftest ]]; then
    selftest
    exit $?
fi

package_exists "$package" || {
    print -u2 -- "profile wm2000 shard: package is not a generated WM2000 shard: $package"
    exit 2
}

if [[ "$mode" == dry-run ]]; then
    print -- "fn64.wm2000-shard-profile.v1 mode=dry-run package=$package target=<fresh-temporary-directory> cargo_jobs=1 max_rss_mib=${FN64_GUARD_MAX_RSS_MIB:-2048} min_free_percent=${FN64_GUARD_MIN_FREE_PERCENT:-40} scope=cold_dependency_graph_plus_shard rom=<ROM>"
    exit 0
fi

if [[ -z ${ROM:-} || ! -f "$ROM" ]]; then
    print -u2 -- "profile wm2000 shard: ROM must name the local NWXE image"
    exit 2
fi
export ROM=${ROM:A}

typeset -r target_dir=$(mktemp -d /tmp/fn64-wm-shard-profile.XXXXXX)
typeset -r guard_jsonl="$target_dir/memory-guard.jsonl"
typeset -r build_log="$target_dir/build.sanitized.log"
typeset -r summary_json="$target_dir/profile.json"

export CARGO_TARGET_DIR=$target_dir
export CARGO_BUILD_JOBS=1
export FN64_PROFILE_BUILD=1
export FN64_GUARD_MAX_RSS_MIB=${FN64_GUARD_MAX_RSS_MIB:-2048}
export FN64_GUARD_MIN_FREE_PERCENT=${FN64_GUARD_MIN_FREE_PERCENT:-40}
export FN64_GUARD_MAX_SECONDS=${FN64_GUARD_MAX_SECONDS:-2400}
export FN64_GUARD_JSONL=$guard_jsonl

set +e
"$repo_root/scripts/memory-guard.zsh" \
    cargo build -j1 --manifest-path "$shard_manifest" -p "$package" 2>&1 \
    | python3 -c \
        'import os, sys
secret = os.environ["ROM"]
for line in sys.stdin:
    sys.stdout.write(line.replace(secret, "<ROM>"))
    sys.stdout.flush()' \
    | tee "$build_log"
typeset -ri build_status=$pipestatus[1]
set -e

typeset guard_elapsed=0
typeset guard_peak=0
typeset guard_free=-1
if [[ -s "$guard_jsonl" ]]; then
    read guard_elapsed guard_peak guard_free <<<"$(tail -n 1 "$guard_jsonl" | sed -E 's/.*"elapsed_seconds":([0-9]+).*"peak_tree_rss_mib":([0-9]+).*"free_percent":(-?[0-9]+).*/\1 \2 \3/')"
fi

if (( build_status != 0 )); then
    print -r -- "{\"schema\":\"fn64.wm2000-shard-profile.v1\",\"package\":\"$package\",\"status\":\"failed\",\"scope\":\"cold_dependency_graph_plus_shard\",\"elapsed_seconds\":$guard_elapsed,\"peak_tree_rss_mib\":$guard_peak,\"final_free_percent\":$guard_free}" > "$summary_json"
    print -u2 -- "profile wm2000 shard: build failed; sanitized log and path-free measurements retained at $target_dir"
    exit $build_status
fi

typeset -a runners
runners=("$target_dir"/debug/build/"$package"-*/out/runner.rs(N))
if (( ${#runners} != 1 )); then
    print -u2 -- "profile wm2000 shard: expected exactly one generated runner, found ${#runners}; target retained at $target_dir"
    exit 3
fi
typeset -r runner=${runners[1]}
typeset -ri source_bytes=$(wc -c < "$runner" | tr -d ' ')
typeset -ri source_lines=$(wc -l < "$runner" | tr -d ' ')
typeset -ri finish_invocations=$(grep -o 'finish!(' "$runner" | wc -l | tr -d ' ')
typeset -i reuse_2k_total_slots=-1
typeset -i reuse_2k_unique_slots=-1
typeset -i reuse_64k_total_slots=-1
typeset -i reuse_64k_unique_slots=-1
typeset -r reuse_line=$(sed -n '/reuse_2k_total_slots=/p' "$build_log" | tail -n 1)
if [[ -n "$reuse_line" ]]; then
    read reuse_2k_total_slots reuse_2k_unique_slots reuse_64k_total_slots reuse_64k_unique_slots \
        <<<"$(reuse_inventory_fields "$reuse_line")"
fi

typeset -r crate_stem=${package//-/_}
typeset -a rlibs
rlibs=("$target_dir"/debug/deps/lib"$crate_stem"-*.rlib(N))
typeset -i rlib_bytes=0
typeset rlib
for rlib in "${rlibs[@]}"; do
    rlib_bytes=$((rlib_bytes + $(stat -f '%z' "$rlib")))
done

print -r -- "{\"schema\":\"fn64.wm2000-shard-profile.v1\",\"package\":\"$package\",\"status\":\"completed\",\"scope\":\"cold_dependency_graph_plus_shard\",\"elapsed_seconds\":$guard_elapsed,\"peak_tree_rss_mib\":$guard_peak,\"final_free_percent\":$guard_free,\"source_bytes\":$source_bytes,\"source_lines\":$source_lines,\"finish_invocations\":$finish_invocations,\"reuse_2k_total_slots\":$reuse_2k_total_slots,\"reuse_2k_unique_slots\":$reuse_2k_unique_slots,\"reuse_64k_total_slots\":$reuse_64k_total_slots,\"reuse_64k_unique_slots\":$reuse_64k_unique_slots,\"rlib_count\":${#rlibs},\"rlib_bytes\":$rlib_bytes}" > "$summary_json"
command cat "$summary_json"
print -- "profile wm2000 shard: fresh target retained at $target_dir"
print -- "profile wm2000 shard: total time includes the cold dependency graph; build-script phase timings are retained in build.sanitized.log"
