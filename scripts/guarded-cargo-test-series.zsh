#!/bin/zsh

# Repeat one focused Cargo test command under the repository memory guard.
# Concurrency fixes need 20+ independent clean runs; keeping the repetition in
# one checked script prevents an ad hoc loop from bypassing the -j1, libtest,
# aggregate-RSS, or free-memory limits.

set -eu

typeset -r repo_root=${0:A:h:h}
typeset -r run_count=${1:-}

if [[ ! $run_count == <1-> ]] || (( run_count == 0 )); then
    print -u2 "usage: $0 RUN_COUNT [cargo-test arguments...]"
    exit 2
fi
shift

typeset run_index=1
while (( run_index <= run_count )); do
    print -u2 "guarded test series: run ${run_index}/${run_count}"
    "$repo_root/scripts/guarded-cargo-test.zsh" "$@"
    (( run_index += 1 ))
done

print -u2 "guarded test series: ${run_count}/${run_count} clean runs"
