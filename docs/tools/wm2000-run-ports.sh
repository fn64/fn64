#!/bin/zsh
set -euo pipefail
NAME=$1; shift
SIB=/private/tmp/md-scratch/sib
AKI=$HOME/Code/aki-recomp
OUT=/private/tmp/md-runs/$NAME
mkdir -p "$OUT/frames"
cd "$SIB/recomps/wm2000"
exec env \
  ROM="$AKI/games/NWXE/wm2000.z64" \
  FN64_ABSENT_N64DD=1 FN64_NO_AUDIO=1 FN64_RENDER=wgpu \
  WM2000_TRACE_PATH="$OUT/trace.jsonl" \
  WM2000_FB_DUMP_DIR="$OUT/frames" \
  WM2000_MAX_STEPS="${WM2000_MAX_STEPS:-4000000}" \
  WM2000_PORTS="${WM2000_PORTS:-2}" \
  /private/tmp/md-scratch/target-ports/release/wm2000-boot "$@"
