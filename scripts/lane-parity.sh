#!/usr/bin/env bash
# lane-parity.sh -- mechanize DESIGN.md section 4's central A/B claim: the C lane
# (FN64_RECOMP=c, N64Recomp-generated C) and the rs lane (FN64_RECOMP=rs, emitted
# typed-Rust whole-ROM crate) link IDENTICAL recompiled semantics, so booting OoT
# through each must produce identical framebuffers swap-for-swap.
#
# Until now that claim was checked exactly once, by hand (docs/OOT-STATUS.md's
# swap-499 SHA). The only gate, examples/oot-boot/tests/boot_depth.rs, asserts a
# liveness floor (>=200 swaps) -- the game could render garbage and still pass.
#
# This is a SCRIPT and not a #[test] on purpose: the two lanes need different
# build.rs configurations (RECOMPILED_DIR + section bridge vs RECOMP_RS_DIR +
# path-dep symlink) and both emit to the SAME target/release/oot-boot path, so a
# single cargo test process cannot hold both. Sequencing builds is the job of a
# script.
#
# Usage:  scripts/lane-parity.sh [SWAPS]     (default 60)
#
# Skips loudly (exit 0) without game content, matching boot_depth.rs's pattern:
# no ROM ships in this repo (README's no-game-content rule).
#
# Exit: 0 = parity held (or skipped), 1 = lanes DIVERGED, 2 = harness error.
#
# Status when written: parity HOLDS through swap 231 and BREAKS at 232.
# Measured (both lanes internally deterministic, 2 identical runs each; the
# divergence also reproduces with FN64_SKIP_AUDIO_UCODE=1, so it is not audio
# timing, and it precedes the default input script's first event at frame 250):
#   swaps 1..231   : framebuffers byte-identical
#   gfx task #232  : C submits 900 tris, rs submits 903  <-- display lists differ
#   framebuffer 234: first differing PNG
#     C : fa81da79cc63c118c329c8bd4e4d750747e59a47deaf2f5ed81dee2740d67d5c
#     rs: 01975db2ca7ed0afa737a8686b4060f455965a5c5219d35e6e4c1e7f58083c2b
# Onset coincides with OoT reaching game_mode=1 (title), entrance=0xcd at swap
# 231 -- i.e. exactly where boot_depth.rs's hand-verified 230-231 depth stopped
# looking, which is why a one-time hand check never saw it.
# So `lane-parity.sh 60` passes today; `lane-parity.sh 240` fails REAL.
set -uo pipefail

SWAPS="${1:-60}"
SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
FN64="${FN64:-$(cd "$SCRIPT_DIR/.." && pwd)}"
BOOT="$FN64/examples/oot-boot"

# Both lanes emit a binary called `oot-boot`, so they MUST NOT share a target
# dir: the second build overwrites the first and the comparison silently
# degenerates into running one lane against itself (which trivially "passes" --
# this bit this script during bring-up). Give each lane its own target dir and
# its own binary path, and refuse to inherit an ambient CARGO_TARGET_DIR, which
# would redirect a build out from under $BIN. FAST-LOOP.md tells jobs to export
# CARGO_TARGET_DIR, so this is the normal environment, not an exotic one.
unset CARGO_TARGET_DIR
C_TARGET="${FN64_PARITY_C_TARGET:-/tmp/fn64-parity-target-c}"
RS_TARGET="${FN64_PARITY_RS_TARGET:-/tmp/fn64-parity-target-rs}"
C_BIN="$C_TARGET/release/oot-boot"
RS_BIN="$RS_TARGET/release/oot-boot"

# --- content gate: skip loudly, exactly like boot_depth.rs -------------------
if [ -z "${FN64_GAME_DIR:-}" ] && { [ -z "${RECOMPILED_DIR:-}" ] || [ -z "${ROM:-}" ]; }; then
  echo "lane-parity: SKIP -- set FN64_GAME_DIR (or RECOMPILED_DIR + ROM) to run the real A/B gate." >&2
  echo "lane-parity: no game content ships in this repo; this gate needs a ROM + recompiled output." >&2
  exit 0
fi
export RECOMPILED_DIR="${RECOMPILED_DIR:-$FN64_GAME_DIR/games/OOTU/RecompiledFuncs}"
export ROM="${ROM:-$FN64_GAME_DIR/games/OOTU/oot-ntsc-1.0.z64}"
for p in "$RECOMPILED_DIR" "$ROM"; do
  [ -e "$p" ] || { echo "lane-parity: SKIP -- missing game content: $p" >&2; exit 0; }
done

FB_GLOB="/tmp/fn64-fb"

# Run one lane to $SWAPS and record "swap<TAB>sha" for every dumped framebuffer.
# The harness only dumps NON-UNIFORM framebuffers, so a blank frame is absent
# from both sides identically -- compare the dumped sets, and their equality.
run_lane() {
  local lane="$1" out="$2" bin="$3"
  [ -x "$bin" ] || { echo "lane-parity: $lane lane binary missing at $bin" >&2; exit 2; }
  rm -f "$FB_GLOB"-*.png
  ( cd "$BOOT" && OOT_MAX_SWAPS="$SWAPS" "$bin" ) >"/tmp/lane-parity-$lane.log" 2>&1
  local observed
  observed=$(grep -oE "VI swaps observed: [0-9]+" "/tmp/lane-parity-$lane.log" | tail -1 | grep -oE "[0-9]+$")
  if [ -z "${observed:-}" ] || [ "$observed" -lt "$SWAPS" ]; then
    echo "lane-parity: FAIL -- $lane lane reached ${observed:-0} swaps, expected $SWAPS (see /tmp/lane-parity-$lane.log)" >&2
    exit 2
  fi
  : > "$out"
  for f in "$FB_GLOB"-*.png; do
    [ -e "$f" ] || continue
    local n; n=$(basename "$f" .png); n=${n#fn64-fb-}
    printf '%s\t%s\n' "$n" "$(shasum -a 256 "$f" | cut -d' ' -f1)" >> "$out"
  done
  sort -n -o "$out" "$out"
  echo "lane-parity: $lane lane reached $observed swaps, dumped $(wc -l < "$out" | tr -d ' ') framebuffers"
}

echo "lane-parity: bounding both lanes at $SWAPS swaps"

# --- C lane ------------------------------------------------------------------
echo "lane-parity: building C lane..."
( cd "$BOOT" && CARGO_TARGET_DIR="$C_TARGET" FN64_RECOMP=c cargo build --release -q ) \
  || { echo "lane-parity: C lane build failed" >&2; exit 2; }
run_lane c /tmp/lane-parity-c.sha "$C_BIN"

# --- rs lane -----------------------------------------------------------------
# The emitted whole-ROM crate is content-addressed and deterministic; reuse it.
echo "lane-parity: emitting/locating rs crate..."
RECOMP_RS_DIR="${RECOMP_RS_DIR:-$("$SCRIPT_DIR/native-emit.sh" 2>/dev/null | tail -1)}"
if [ -z "${RECOMP_RS_DIR:-}" ] || [ ! -f "$RECOMP_RS_DIR/src/lib.rs" ]; then
  echo "lane-parity: SKIP -- no emitted rs crate (scripts/native-emit.sh produced nothing usable)." >&2
  exit 0
fi
export RECOMP_RS_DIR
# build.rs asserts this symlink resolves to RECOMP_RS_DIR (see its rs-lane branch).
link="$BOOT/rs/recompiled"
if [ -e "$link" ] && [ ! -L "$link" ]; then
  echo "lane-parity: $link exists and is not a symlink; refusing to replace it" >&2; exit 2
fi
ln -sfn "$RECOMP_RS_DIR" "$link" || exit 2
echo "lane-parity: building rs lane (large emitted crate; first build is slow)..."
CARGO_TARGET_DIR="$RS_TARGET" FN64_RECOMP=rs \
  cargo build --manifest-path "$BOOT/rs/Cargo.toml" --release -q \
  || { echo "lane-parity: rs lane build failed" >&2; exit 2; }

# Self-check: the whole experiment is void if both "lanes" are the same binary.
# Identical bytes here means a build was redirected and we would be comparing a
# lane against itself -- a green that proves nothing. Trap it loudly.
if [ "$(shasum -a 256 "$C_BIN" | cut -d' ' -f1)" = "$(shasum -a 256 "$RS_BIN" | cut -d' ' -f1)" ]; then
  echo "lane-parity: FAIL -- the C and rs binaries are byte-identical ($C_BIN, $RS_BIN)." >&2
  echo "lane-parity: a lane build was redirected; this comparison would be a lane against itself." >&2
  exit 2
fi
run_lane rs /tmp/lane-parity-rs.sha "$RS_BIN"

# --- compare -----------------------------------------------------------------
if diff -q /tmp/lane-parity-c.sha /tmp/lane-parity-rs.sha >/dev/null; then
  echo "lane-parity: OK -- C and rs lanes byte-identical across all dumped framebuffers (<= swap $SWAPS)"
  exit 0
fi

echo "lane-parity: DIVERGED -- the two recomp lanes do NOT link identical semantics." >&2
first=$(join -t"$(printf '\t')" /tmp/lane-parity-c.sha /tmp/lane-parity-rs.sha \
        | awk -F'\t' '$2!=$3 {print $1; exit}')
echo "lane-parity: first divergent swap: ${first:-<set of dumped swaps differs between lanes>}" >&2
if [ -n "${first:-}" ]; then
  echo "  C : $(awk -F'\t' -v s="$first" '$1==s{print $2}' /tmp/lane-parity-c.sha)" >&2
  echo "  rs: $(awk -F'\t' -v s="$first" '$1==s{print $2}' /tmp/lane-parity-rs.sha)" >&2
fi
echo "lane-parity: full per-swap SHAs: /tmp/lane-parity-{c,rs}.sha" >&2
exit 1
