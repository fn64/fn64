#!/bin/zsh
# Run the prebuilt wm2000-boot from THIS lane's own scratch tree.
# Every run gets its own trace path and fb dump dir (the swap-1901 collision trap).
set -euo pipefail
NAME=$1; shift
SIB=/private/tmp/md-scratch/sib
AKI=$HOME/Code/aki-recomp
OUT=/private/tmp/md-runs/$NAME
mkdir -p "$OUT/frames"
cd "$SIB/recomps/wm2000"
exec env \
  ROM="$AKI/games/NWXE/wm2000.z64" \
  FN64_ABSENT_N64DD=1 \
  FN64_NO_AUDIO=1 \
  FN64_RENDER=wgpu \
  WM2000_TRACE_PATH="$OUT/trace.jsonl" \
  WM2000_FB_DUMP_DIR="$OUT/frames" \
  WM2000_MAX_STEPS="${WM2000_MAX_STEPS:-3000000}" \
  ./packages/wm2000-boot/rs/target/release/wm2000-boot "$@"
# Usage: WM2000_MAX_STEPS=4000000 WM2000_INPUT_SCRIPT="..." ./wm2000-run-isolated.sh <name>
# Every run gets its OWN trace path and fb dump dir under /private/tmp/md-runs/<name>.
# Sharing either is the collision that counterfeited a plateau at swap 1901.
