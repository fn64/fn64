#!/bin/zsh

# Safe local nextest feedback loop. Nextest's -j1 serializes test processes,
# but unlike `cargo test -j1` it does not constrain Cargo's compile jobs.
# Bind both lanes explicitly before entering the process-group memory guard.

set -eu

typeset -r repo_root=${0:A:h:h}

export FN64_GUARD_MAX_RSS_MIB=${FN64_GUARD_MAX_RSS_MIB:-4096}
export FN64_GUARD_MIN_FREE_PERCENT=${FN64_GUARD_MIN_FREE_PERCENT:-40}
export CARGO_BUILD_JOBS=1

exec "$repo_root/scripts/memory-guard.zsh" cargo nextest run -j1 "$@"
