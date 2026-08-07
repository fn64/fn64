#!/bin/zsh
# Scratch observational runner for extending entrance-to-match.schedule.
#
# Unlike render-benchmark.zsh this has NO load gate (it is not a measurement)
# and it always dumps frames. Usage:
#   probe-route.zsh <schedule> <dumpdir> [steps] [first_task] [limit]

set -uo pipefail

SCHED="$1"
DUMP="$2"
STEPS="${3:-1500000}"
FIRST_TASK="${4:-0}"
LIMIT="${5:-4000}"

HERE="${0:A:h}"
REPO="${HERE:h:h}"
cd "$REPO" || exit 1

source .claude/local.env
export ROM="${ROM:-$FN64_DISCOVER_NWXE_ROM}"

C="${FN64_CAPTURES_DIR:-$HOME/Code/aki-recomp/captures}"
G="$C/wm-general-exception-images"
export FN64_EXECUTABLE_IMAGES="$G/run-1/image.json:$G/run-2/image.json:$G/run-3/image.json"
export FN64_BOOT_CONTEXT="${FN64_BOOT_CONTEXT:-$C/wm2000-boot-context.json}"

export FN64_ABSENT_N64DD=1
export FN64_BLOCK_CONTINUE_AFTER_OVERLAY=1
export FN64_MPROTECT_BARRIER=1
export FN64_CONTROLLER_SCHEDULE="$SCHED"
export FN64_BLOCK_MAX_STEPS="$STEPS"
export FN64_HEARTBEAT=${FN64_HEARTBEAT:-200000}

mkdir -p "$DUMP"
export FN64_RENDER_DUMP_DIR="$DUMP"
export FN64_RENDER_DUMP_FIRST_TASK="$FIRST_TASK"
export FN64_RENDER_DUMP_LIMIT="$LIMIT"

BINARY="$REPO/examples/wm2000-block-boot/target/release/wm2000-block-boot"
print "schedule: $SCHED"
print "dump:     $DUMP (skip $FIRST_TASK, limit $LIMIT)"
"$BINARY" 2>&1 | tee "/tmp/fn64-probe-$(basename $DUMP).log" | grep -E --line-buffered \
    'controller|port0_reads|render_error|heartbeat|done'
