#!/bin/zsh
# Run the ALREADY-BUILT rs-lane binary. run-lane.sh rebuilds the emitted crate
# and rsyncs a scratch sibling tree on every invocation, which makes concurrent
# runs race on that shared tree (observed: four parallel probes all died on
# `cp: .../recomps/wm2000: File exists`). Probing needs many runs of one fixed
# binary, so the build belongs outside the loop.
#
# WM2000_NO_TRACE is deliberately ABSENT so framebuffer dumps are armed; the
# harness checks it with is_some(), so setting it to 0 would still disable them.
set -euo pipefail
SIB=${SIB:-/private/tmp/wm2000-probe-run/scratch/sib}
AKI=${AKI:-$HOME/Code/aki-recomp}
cd "$SIB/recomps/wm2000"
exec env \
  ROM="$AKI/games/NWXE/wm2000.z64" \
  FN64_ABSENT_N64DD=1 \
  FN64_NO_AUDIO=1 \
  FN64_RENDER="${FN64_RENDER:-wgpu}" \
  WM2000_TRACE_PATH="${WM2000_TRACE_PATH:-/tmp/wm2000-trace-$$.jsonl}" \
  WM2000_MAX_STEPS="${WM2000_MAX_STEPS:-700000}" \
  ./packages/wm2000-boot/rs/target/release/wm2000-boot "$@"
