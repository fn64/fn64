#!/bin/zsh
# WM2000 boot ladder: a REGRESSION GATE, not an exploration tool.
#
# Every blocker cleared in this project was verified once, on one run, and then
# never re-checked. Nothing would catch a later change silently dropping the ROM
# from swap 17,473 back to 3,000 -- it would only surface the next time somebody
# happened to run the game. This turns that from luck into a gate.
#
# The contract: with the committed two-controller lead-in, WM2000 must reach at
# least FLOOR VI swaps with ZERO raw-DPC backend errors and ZERO panics. Raise
# FLOOR when a fix legitimately advances the ROM; never lower it to make a run
# pass. Lowering it is how a regression ships.
#
#   docs/tools/wm2000-boot-ladder.sh [floor]
#
# Exit 0 = gate held. Exit 1 = regression (or the run failed to start).
set -euo pipefail

FN64=${FN64:-/private/tmp/fn64-rt64-main-integration}
FLOOR=${1:-${WM2000_LADDER_FLOOR:-17000}}
RUN_ID="ladder-$$"
SCRATCH_ROOT=${WM2000_LADDER_SCRATCH:-/private/tmp/$RUN_ID}
LOG="$SCRATCH_ROOT.log"
LEADIN=${WM2000_LADDER_LEADIN:-$FN64/docs/tools/wm2000-match-leadin.txt}

if [[ ! -f "$LEADIN" ]]; then
  echo "[ladder] FATAL: lead-in not found at $LEADIN" >&2
  exit 1
fi

# The ladder needs ONE number -- the last vi_swaps line -- and no pictures, so
# it runs with the harness's own WM2000_NO_TRACE=1 intact. Measured cost of not
# doing that: a traced run wrote a 996 MB JSONL sink and ran roughly three
# times slower for output the gate never reads. Frame dumps are disabled along
# with the trace (dumps_disabled = trace_disabled || ...), which is exactly
# what this gate wants. A run that DOES want frames should grep the flag out
# itself and supply its own WM2000_TRACE_PATH, never share the default sink --
# a shared sink once made four concurrent runs abort at an identical swap and
# counterfeit a plateau.
RUNNER=$SCRATCH_ROOT-run.sh
mkdir -p "$SCRATCH_ROOT"
cp "$HOME/Code/recomps/wm2000/packages/wm2000-boot/rs/run-rs-lane.sh" "$RUNNER"

echo "[ladder] floor=$FLOOR  scratch=$SCRATCH_ROOT"
set +e
FN64="$FN64" \
SCRATCH="$SCRATCH_ROOT/scratch" \
WM2000_PORTS=2 \
WM2000_INPUT_SCRIPT="$(cat "$LEADIN")" \
WM2000_MAX_STEPS=${WM2000_LADDER_STEPS:-8000000} \
  zsh "$RUNNER" > "$LOG" 2>&1
set -e

SWAPS=$(grep -oE 'vi_swaps=[0-9]+' "$LOG" | tail -1 | grep -oE '[0-9]+' || echo 0)
PANICS=$(grep -c 'panicked at' "$LOG" || true)
BACKEND=$(grep -c 'backend error' "$LOG" || true)

echo "[ladder] reached vi_swaps=$SWAPS  panics=$PANICS  backend_errors=$BACKEND"

FAIL=0
if [[ "$PANICS" -ne 0 ]]; then
  echo "[ladder] FAIL: $PANICS panic(s)" >&2
  grep -m1 'panicked at' -A1 "$LOG" >&2 || true
  FAIL=1
fi
if [[ "$BACKEND" -ne 0 ]]; then
  echo "[ladder] FAIL: $BACKEND raw-DPC backend error(s)" >&2
  grep -m1 'backend error' "$LOG" | head -c 300 >&2; echo >&2
  FAIL=1
fi
if [[ "$SWAPS" -lt "$FLOOR" ]]; then
  echo "[ladder] FAIL: reached $SWAPS, floor is $FLOOR -- REGRESSION" >&2
  FAIL=1
fi

# A clean run is necessary but not sufficient: a packet whose commands were all
# dropped upstream can report success with zero pixels, so "zero backend errors"
# is not "the frame is right". The differential runner is what checks pixels;
# this gate only proves the ROM still gets as far as it did.
if [[ "$FAIL" -eq 0 ]]; then
  echo "[ladder] PASS: $SWAPS >= $FLOOR, clean"
fi
exit "$FAIL"
