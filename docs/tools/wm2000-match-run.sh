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
# WM2000_MODE=grapple (default) drives the in-match cycle throughout;
# WM2000_MODE=leadin-only stops pressing at swap 6000, which is the CONTROL arm
# of the input differential -- identical lead-in, nothing after it.
SCHED_MODE=${WM2000_MODE:-grapple}
SCRIPT=${WM2000_SCHEDULE:-$(python3 "$FN64/docs/tools/wm2000-schedule.py" --mode "$SCHED_MODE" --until 200000)}
# Default watch set, all read out of the ROM (see RT64-WM2000-MATCH-GRAMMAR.md):
#
#   0x80095184/86/90/92  both plugged ports' HELD/PRESSED, in the per-port
#                        record D_80095180 (stride 0xC) that the GAMEPLAY code
#                        reads -- proves an injected button arrived where the
#                        game looks for it, independent of the renderer.
#   0x8016ED2A (u8)      THE MATCH-END FLAG. bit 0x80 set == match over: the
#                        frame loop's only call to the exiting fade
#                        func_800EE4AC is gated on it at 0x800E1C9C, and that
#                        fade is what sets the loop-exit register $s4.
#   0x801589D2 (s16)     post-match sequence counter, ticked in func_801229E0;
#                        func_80122AF4 sets 0x8016ED2A bit 0x80 once it passes
#                        0x7530 (slti at 0x80122AFC).
#   0x801589D0 (s16)     the adjacent intro/entrance timer (slti 0x1F) -- watched
#                        so it is not mistaken for the match clock.
#   0x801567B0           referee-count / display index (func_800E9D8C).
: ${WM2000_WATCH:=0x80095184,0x80095186,0x80095190,0x80095192,0x8016ED2A:1,0x801589D2,0x801589D0,0x801567B0}

mkdir -p "$ROOT/frames"
RUNNER=$ROOT/run.sh
cp "$HOME/Code/recomps/wm2000/packages/wm2000-boot/rs/run-rs-lane.sh" "$RUNNER"

# run-rs-lane.sh copies the harness from the sibling repo into $SCRATCH/sib on
# every run, so the WM2000_WATCH guest-memory probe has to be re-applied after
# that copy and before the build. Splice the patcher in at the copy site rather
# than editing the sibling repo (which fn64 lanes do not own).
/usr/bin/sed -i '' \
  's|^ln -sfn "\$EMIT" \(.*\)$|python3 '"$FN64"'/docs/tools/wm2000-watch-patch.py "$SIB/recomps/wm2000/packages/wm2000-boot/src/main.rs"\
ln -sfn "$EMIT" \1|' "$RUNNER"
grep -q wm2000-watch-patch "$RUNNER" || { echo "[match-run] FATAL: watch probe not spliced into runner" >&2; exit 1; }

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

echo "[match-run] mode=$MODE sched=$SCHED_MODE name=$NAME steps=$STEPS root=$ROOT"
echo "[match-run] watch=$WM2000_WATCH"
echo "[match-run] schedule bytes=${#SCRIPT} last=${SCRIPT##*;}"

set +e
env FN64="$FN64" SCRATCH="$ROOT/scratch" \
  WM2000_PORTS=${WM2000_PORTS:-2} \
  WM2000_INPUT_SCRIPT="$SCRIPT" \
  WM2000_MAX_STEPS="$STEPS" \
  ${WM2000_WATCH:+WM2000_WATCH=$WM2000_WATCH} \
  ${WM2000_STOP_AT_SWAP:+WM2000_STOP_AT_SWAP=$WM2000_STOP_AT_SWAP} \
  "${EXTRA[@]}" \
  zsh "$RUNNER" > "$LOG" 2>&1
RC=$?
set -e

echo "[match-run] rc=$RC"
echo "[match-run] last progress: $(grep -oE 'vi_swaps=[0-9]+ gfx_tasks=[0-9]+ audio_tasks=[0-9]+' "$LOG" | tail -1)"
echo "[match-run] termination: $(grep -E 'step budget|BOOT SUMMARY|STOP_AT_SWAP' "$LOG" | tail -1)"
echo "[match-run] watch changes: $(grep -c 'wm2000-watch. swap' "$LOG" || true)"
echo "[match-run] panics=$(grep -c 'panicked at' "$LOG" || true) backend_errors=$(grep -c 'backend error' "$LOG" || true)"
echo "[match-run] log=$LOG"
