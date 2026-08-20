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
# Port 1 fights independently unless WM2000_SOLO=1. It gets no lead-in (port 0
# navigates the menus; a second pad confirming on the same frames would
# double-advance them) and a rotated, phase-shifted cycle, so the two wrestlers
# are not performing identical moves on identical frames -- which is the shape
# of a stalemate rather than a fight.
if [[ "$SCHED_MODE" == "grapple" && -z "${WM2000_SOLO:-}" ]]; then
  : ${WM2000_INPUT_SCRIPT_P1:=$(python3 "$FN64/docs/tools/wm2000-schedule.py" --port1 --until 200000)}
fi
# Default watch set, all read out of the ROM (see RT64-WM2000-MATCH-GRAMMAR.md):
#
#   0x80095184/86/90/92  both plugged ports' HELD/PRESSED, in the per-port
#                        record D_80095180 (stride 0xC) that the GAMEPLAY code
#                        reads -- proves an injected button arrived where the
#                        game looks for it, independent of the renderer.
#   0x801589D6 (u8)      THE MATCH STATE. func_801226A0 switches on it through
#                        jtbl_80151970: 0 init, 1 entrance, 2 LIVE MATCH,
#                        3 decision (one frame, picks the winner), 4 post-match.
#                        This is the single best progress probe: 2 -> 3 IS the
#                        match ending.
#   0x8016ED2A (u8)      the end flags. bit 0x40 normal finish, bit 0x10
#                        time-limit draw, bit 0x80 sequence over -> the frame
#                        loop's only call to the exiting fade func_800EE4AC
#                        (gated at 0x800E1C9C) which sets loop-exit $s4.
#   0x801589D2 (s16)     post-match sequence counter (state 4), ticked in
#                        func_801229E0; func_80122AF4 sets bit 0x80 once it
#                        passes 0x7530.
#   0x801589D4 (s16)     the WINNER index, stored in state 3 from func_80127388.
#   0x8016F0AC/0x80166F88 the match clock, ticked every 30 frames by
#                        func_801444E0 inside the time-limit check func_80123D64.
#                        The match ends when 0x8016F0AC reaches
#                        D_8014E1C4[D_800961D2].
#   0x8016ECC0 (s8)      the referee count, loaded from D_8014E198 = {0,10,20,0}
#                        by match type; counts DOWN to 0. (D_801567B0/B2 are
#                        HUD digits only and were refuted as the counter.)
#   0x801589E6 (s16)     THE LIVE PIN COUNT, against the target 0x801589E4
#                        (advanced 0x801231E8, compared 0x80123238). Nonzero
#                        only while a pin is actually in progress, which makes
#                        it the cheapest way to tell "the wrestlers are really
#                        fighting" from "the wrestlers are milling about".
#   0x801671F0 (s16)     player-0 spirit/health (record base 0x801671E2, stride
#                        0x104); tested slti 0x32 at 0x801239DC.
#   0x801672F4           player-1 spirit/health (0x801671F0 + 0x104).
#   0x800961D2           the time-limit SETTING, the index into D_8014E1C4.
#   0x8014E1C4           entry 0 of the time-limit table -- so a run reports the
#                        configured bound instead of anyone having to guess
#                        whether the wait is 3 minutes or 60.
: ${WM2000_WATCH:=0x80095184,0x80095186,0x80095190,0x80095192,0x801589D6:1,0x8016ED2A:1,0x801589D2,0x801589D4,0x8016F0AC,0x80166F88,0x8016ECC0:1,0x801589E6,0x801589E4,0x801671F0,0x801672F4,0x800961D2,0x8014E1C4}

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
echo "[match-run] port1 independent: ${WM2000_INPUT_SCRIPT_P1:+yes (${#WM2000_INPUT_SCRIPT_P1} bytes)}${WM2000_INPUT_SCRIPT_P1:-no (mirrored)}"
echo "[match-run] schedule bytes=${#SCRIPT} last=${SCRIPT##*;}"

set +e
env FN64="$FN64" SCRATCH="$ROOT/scratch" \
  WM2000_PORTS=${WM2000_PORTS:-2} \
  WM2000_INPUT_SCRIPT="$SCRIPT" \
  WM2000_MAX_STEPS="$STEPS" \
  ${WM2000_WATCH:+WM2000_WATCH=$WM2000_WATCH} \
  ${WM2000_INPUT_SCRIPT_P1:+WM2000_INPUT_SCRIPT_P1=$WM2000_INPUT_SCRIPT_P1} \
  ${WM2000_STOP_AT_SWAP:+WM2000_STOP_AT_SWAP=$WM2000_STOP_AT_SWAP} \
  "${EXTRA[@]}" \
  zsh "$RUNNER" > "$LOG" 2>&1
RC=$?
set -e

echo "[match-run] rc=$RC"
echo "[match-run] ---- match state machine (D_801589D6: 2=live 3=decision 4=post) ----"
grep -E '0x801589d6' "$LOG" | tail -8 || echo "  (no state transitions observed)"
echo "[match-run] ---- end flags 0x8016ED2A (0x40 finish, 0x10 draw, 0x80 fade) ----"
grep -E '0x8016ed2a' "$LOG" | tail -6 || echo "  (never changed)"
echo "[match-run] ---- match clock / limit ----"
grep -E '0x8016f0ac|0x800961d2|0x8014e1c4' "$LOG" | tail -6 || echo "  (no clock ticks)"
echo "[match-run] ---- winner index 0x801589D4 ----"
grep -E '0x801589d4' "$LOG" | tail -3 || echo "  (never set)"
echo "[match-run] ---- gameplay-visible input (ports 0/1 HELD+PRESSED) ----"
echo "  port0 changes: $(grep -cE '0x80095184|0x80095186' "$LOG" || true)   port1 changes: $(grep -cE '0x80095190|0x80095192' "$LOG" || true)"
echo "[match-run] last progress: $(grep -oE 'vi_swaps=[0-9]+ gfx_tasks=[0-9]+ audio_tasks=[0-9]+' "$LOG" | tail -1)"
echo "[match-run] termination: $(grep -E 'step budget|BOOT SUMMARY|STOP_AT_SWAP' "$LOG" | tail -1)"
echo "[match-run] watch changes: $(grep -c 'wm2000-watch. swap' "$LOG" || true)"
echo "[match-run] panics=$(grep -c 'panicked at' "$LOG" || true) backend_errors=$(grep -c 'backend error' "$LOG" || true)"
echo "[match-run] log=$LOG"
