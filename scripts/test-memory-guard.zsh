#!/bin/zsh

# Shell-only safety regressions for memory-guard.zsh. Fixtures are deliberately
# short and allocate no meaningful memory; this script never invokes Cargo.

set -u
unsetopt BG_NICE

typeset -r script_path=${0:A}
typeset -r repo_root=${0:A:h:h}
typeset -r guard=$repo_root/scripts/memory-guard.zsh

case ${1:-} in
    reparent-fixture)
        (sleep 1; touch -- "$2") &
        exit 0
        ;;
    stubborn-fixture)
        (trap '' TERM; sleep 5; touch -- "$2") &
        exit 0
        ;;
esac

typeset test_dir
test_dir=$(mktemp -d "${TMPDIR:-/tmp}/fn64-memory-guard-test.XXXXXX") || exit 1
cleanup() {
    rm -f -- "$test_dir/reparent-finished" "$test_dir/stubborn-escaped" "$test_dir/samples.jsonl"
    rmdir -- "$test_dir" 2>/dev/null || true
}
trap cleanup EXIT

run_guard() {
    FN64_GUARD_MAX_RSS_MIB=1024 \
    FN64_GUARD_MIN_FREE_PERCENT=0 \
    FN64_GUARD_REPORT_INTERVAL=100 \
        "$guard" "$@"
}

# Preserve the launched command's status when its group exits normally.
run_guard /bin/sh -c 'exit 7'
typeset -ri status_exit=$?
if (( status_exit != 7 )); then
    print -u2 -- "memory guard self-test: expected command exit 7, got $status_exit"
    exit 1
fi

# The direct child exits immediately, leaving a reparented group member. The
# guard must not return until that member has completed its observable action.
run_guard "$script_path" reparent-fixture "$test_dir/reparent-finished"
typeset -ri reparent_exit=$?
if (( reparent_exit != 0 )) || [[ ! -f "$test_dir/reparent-finished" ]]; then
    print -u2 -- "memory guard self-test: reparented descendant escaped lifetime tracking"
    exit 1
fi

# A TERM-resistant reparented member must be killed with its exact group at the
# wall-time boundary. Waiting past its own timer proves it did not survive.
FN64_GUARD_MAX_SECONDS=1 run_guard "$script_path" stubborn-fixture "$test_dir/stubborn-escaped"
typeset -ri timeout_exit=$?
if (( timeout_exit != 124 )); then
    print -u2 -- "memory guard self-test: expected timeout exit 124, got $timeout_exit"
    exit 1
fi
sleep 3
if [[ -e "$test_dir/stubborn-escaped" ]]; then
    print -u2 -- "memory guard self-test: TERM-resistant descendant survived group KILL"
    exit 1
fi

# JSONL is deliberately path-free even when the command receives a private
# path. Stable V1 field names remain compatible with existing profile readers.
FN64_GUARD_JSONL=$test_dir/samples.jsonl run_guard /usr/bin/true "$test_dir/private-rom-name.z64"
typeset -ri json_exit=$?
if (( json_exit != 0 )) || ! grep -q '"schema":"fn64.memory-guard.sample.v1"' "$test_dir/samples.jsonl"; then
    print -u2 -- "memory guard self-test: JSONL sample missing"
    exit 1
fi
if grep -q 'private-rom-name\|fn64-memory-guard-test' "$test_dir/samples.jsonl"; then
    print -u2 -- "memory guard self-test: JSONL disclosed a command argument or path"
    exit 1
fi

# Short, bounded helper phases may request denser sampling to avoid the
# default one-second completion floor without weakening RSS/free-memory checks.
FN64_GUARD_POLL_SECONDS=0.1 run_guard /usr/bin/true
typeset -ri fast_poll_exit=$?
if (( fast_poll_exit != 0 )); then
    print -u2 -- "memory guard self-test: fast polling rejected a valid command"
    exit 1
fi
FN64_GUARD_POLL_SECONDS=0.01 run_guard /usr/bin/true
typeset -ri invalid_poll_exit=$?
if (( invalid_poll_exit != 2 )); then
    print -u2 -- "memory guard self-test: invalid polling interval was accepted"
    exit 1
fi

print -- "memory guard self-test: PASS"
