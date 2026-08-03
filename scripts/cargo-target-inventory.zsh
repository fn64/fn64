#!/bin/zsh

# Read-only inventory for explicitly named Cargo target directories. This
# script has no clean/prune mode and never removes artifacts.

set -eu

if (( $# == 0 )); then
    print -u2 -- "usage: $0 TARGET_DIR [TARGET_DIR ...]"
    exit 2
fi

size_kib() {
    local inventory_path=$1
    if [[ -e "$inventory_path" ]]; then
        du -sk "$inventory_path" | awk '{print $1}'
    else
        print -- 0
    fi
}

for target_argument in "$@"; do
    typeset target=${target_argument:A}
    if [[ "$target" == / || ! -d "$target" ]]; then
        print -u2 -- "cargo target inventory: expected an explicit target directory, got $target_argument"
        exit 2
    fi

    typeset -i total_kib=$(size_kib "$target")
    typeset -i build_kib=$(size_kib "$target/debug/build")
    typeset -i deps_kib=$(size_kib "$target/debug/deps")
    typeset -i incremental_kib=$(size_kib "$target/debug/incremental")
    typeset -i incremental_generations=0
    typeset -i runner_copies=0
    typeset -i runner_bytes=0
    typeset -i shard_rlibs=0
    typeset -i shard_rlib_bytes=0

    if [[ -d "$target/debug/incremental" ]]; then
        incremental_generations=$(find "$target/debug/incremental" -mindepth 1 -maxdepth 1 -type d -print | wc -l | tr -d ' ')
    fi
    if [[ -d "$target/debug/build" ]]; then
        read runner_copies runner_bytes <<<"$(find "$target/debug/build" -type f -path '*/out/runner.rs' -exec stat -f '%z' {} \; | awk '{bytes += $1; count += 1} END {print count + 0, bytes + 0}')"
    fi
    if [[ -d "$target/debug/deps" ]]; then
        read shard_rlibs shard_rlib_bytes <<<"$(find "$target/debug/deps" -maxdepth 1 -type f -name 'libwm2000_block_*.rlib' -exec stat -f '%z' {} \; | awk '{bytes += $1; count += 1} END {print count + 0, bytes + 0}')"
    fi

    print -- "cargo_target path=$target total_mib=$((total_kib / 1024)) build_mib=$((build_kib / 1024)) deps_mib=$((deps_kib / 1024)) incremental_mib=$((incremental_kib / 1024)) incremental_generations=$incremental_generations runner_copies=$runner_copies runner_mib=$((runner_bytes / 1024 / 1024)) shard_rlibs=$shard_rlibs shard_rlib_mib=$((shard_rlib_bytes / 1024 / 1024))"

    if [[ -d "$target/debug/build" ]]; then
        find "$target/debug/build" -mindepth 1 -maxdepth 1 -type d -print \
            | sed -E 's#^.*/##; s/-[0-9a-f]{16}$//' \
            | sort \
            | uniq -c \
            | sort -nr \
            | head -10 \
            | awk '{printf "cargo_target_build_generations package=%s count=%d\n", $2, $1}'
    fi
done
