#!/bin/zsh

# Safe local Cargo-test feedback loop. Cargo's -j1 limits compiler jobs but
# does not serialize the libtest binary; RUST_TEST_THREADS closes that second
# concurrency lane before the process-group memory guard starts monitoring it.

set -eu

typeset -r repo_root=${0:A:h:h}

export FN64_GUARD_MAX_RSS_MIB=${FN64_GUARD_MAX_RSS_MIB:-4096}
export FN64_GUARD_MIN_FREE_PERCENT=${FN64_GUARD_MIN_FREE_PERCENT:-40}
export CARGO_BUILD_JOBS=1
export RUST_TEST_THREADS=${RUST_TEST_THREADS:-1}

exec "$repo_root/scripts/memory-guard.zsh" cargo test -j1 "$@"
