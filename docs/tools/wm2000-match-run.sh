#!/bin/zsh
# Run WM2000 far enough to find out whether a MATCH ENDS.
#
# Two configurations, because the instruments have opposite costs:
#
#   --long   WM2000_NO_TRACE=1, no frames, huge step budget. This is the run
#            that answers "how far does it get and does it terminate". A traced
#            run buffers every executor event IN MEMORY (write_trace_file reads
#            fn64_abi::copy_trace() at exit) as well as appending JSONL, so a
#            40M-step traced run is a memory bomb, not merely a disk one.
#
#   --frames trace sink armed (dumps are gated on it: dumps_disabled =
#            trace_disabled || NO_DUMP), bounded by WM2000_STOP_AT_SWAP so the
#            sink stays finite. This is the run that produces pictures.
#
# Every run gets its OWN scratch root, trace path and dump dir: a shared sink
# is what counterfeited a plateau at swap 1901 (RT64-WM2000-HARNESS-TRAPS.md).
set -euo pipefail

FN64=${FN64:-/private/tmp/fn64-match-end}
MODE=${1:?usage: wm2000-match-run.sh --long|--frames <name>}
NAME=${2:?usage: wm2000-match-run.sh --long|--frames <name>}
ROOT=/private/tmp/match-end/$NAME
LOG=$ROOT.log
SCRIPT=${WM2000_SCHEDULE:-$(python3 "$FN64/docs/tools/wm2000-schedule.py" --mode grapple --until 200000)}

mkdir -p "$ROOT/frames"
RUNNER=$ROOT/run.sh
cp "$HOME/Code/recomps/wm2000/packages/wm2000-boot/rs/run-rs-lane.sh" "$RUNNER"

if [[ "$MODE" == "--frames" ]]; then
  # Strip the harness's own WM2000_NO_TRACE=1 -- it also disables the PNG dumps
  # (dumps_disabled = trace_disabled || ...), which is the documented trap.
  /usr/bin/sed -i '' '/WM2000_NO_TRACE=1/d' "$RUNNER"
  EXTRA=(WM2000_TRACE_PATH="$ROOT/trace.jsonl" WM2000_FB_DUMP_DIR="$ROOT/frames")
  STEPS=${WM2000_MAX_STEPS:-12000000}
else
  EXTRA=()
  STEPS=${WM2000_MAX_STEPS:-40000000}
fi

echo "[match-run] mode=$MODE name=$NAME steps=$STEPS root=$ROOT"
echo "[match-run] schedule bytes=${#SCRIPT} last=${SCRIPT##*;}"

set +e
env FN64="$FN64" SCRATCH="$ROOT/scratch" \
  WM2000_PORTS=${WM2000_PORTS:-2} \
  WM2000_INPUT_SCRIPT="$SCRIPT" \
  WM2000_MAX_STEPS="$STEPS" \
  ${WM2000_STOP_AT_SWAP:+WM2000_STOP_AT_SWAP=$WM2000_STOP_AT_SWAP} \
  "${EXTRA[@]}" \
  zsh "$RUNNER" > "$LOG" 2>&1
RC=$?
set -e

echo "[match-run] rc=$RC"
echo "[match-run] last progress: $(grep -oE 'vi_swaps=[0-9]+ gfx_tasks=[0-9]+ audio_tasks=[0-9]+' "$LOG" | tail -1)"
echo "[match-run] termination: $(grep -E 'step budget|BOOT SUMMARY|STOP_AT_SWAP' "$LOG" | tail -1)"
echo "[match-run] panics=$(grep -c 'panicked at' "$LOG" || true) backend_errors=$(grep -c 'backend error' "$LOG" || true)"
echo "[match-run] log=$LOG"
