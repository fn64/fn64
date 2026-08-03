#!/usr/bin/env bash
# lane-parity.sh -- compare the C lane (FN64_RECOMP=c, N64Recomp-generated C)
# and rs lane (FN64_RECOMP=rs, emitted typed-Rust whole-ROM crate) without
# granting the legacy C corpus authority it has not mechanically earned.
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
# Usage:  scripts/lane-parity.sh [--observe] [SWAPS]     (default 60)
#         scripts/lane-parity.sh --dry-run [--observe] [SWAPS]
#         scripts/lane-parity.sh --selftest
#
# Default mode first compares the generated callable-body inventories and
# refuses to run a semantic framebuffer gate if a callable C empty body has a
# nonempty Rust counterpart. `--observe` still runs the framebuffer comparison,
# but labels it observational compatibility rather than semantic parity.
#
# Skips loudly (exit 0) without game content, matching boot_depth.rs's pattern:
# no ROM ships in this repo (README's no-game-content rule).
#
# Exit: 0 = authoritative parity held, or observation matched (or skipped),
#       1 = observed framebuffers diverged, 2 = authority/harness error.
#
# Current measured observation: framebuffers MATCH through swap 60. Both boot harnesses now
# advance virtual time only after guest quiescence. An earlier apparent swap-10
# mismatch came from the old "advance after 100 resumes" policy: the rs lane has
# denser host checkpoints than generated C, so the two guests reached different
# work before the same numbered swap. It was harness checkpoint-density skew,
# not a renderer or recompiler semantic difference.
#
# The mechanical preflight currently finds callable empty C bodies with real
# Rust counterparts, so default mode rejects C authority at swap zero. A
# historical deeper observation first differed at framebuffer 234 after gfx
# task 232 reached that limitation. The observation says where output first
# differed in that run; it does not prove the empty bodies were irrelevant
# earlier.
set -uo pipefail

OBSERVE=0
MODE=run
SWAPS=60
SWAPS_SET=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    --observe)
      [ "$OBSERVE" -eq 0 ] || { echo "lane-parity: duplicate --observe" >&2; exit 2; }
      OBSERVE=1
      ;;
    --dry-run)
      [ "$MODE" = run ] || { echo "lane-parity: duplicate or conflicting mode" >&2; exit 2; }
      MODE=dry-run
      ;;
    --selftest)
      [ "$MODE" = run ] || { echo "lane-parity: duplicate or conflicting mode" >&2; exit 2; }
      MODE=selftest
      ;;
    *)
      if [[ "$1" =~ ^[1-9][0-9]*$ ]] && [ "$SWAPS_SET" -eq 0 ]; then
        SWAPS="$1"
        SWAPS_SET=1
      else
        echo "usage: scripts/lane-parity.sh [--observe] [SWAPS] | --dry-run [--observe] [SWAPS] | --selftest" >&2
        exit 2
      fi
      ;;
  esac
  shift
done
SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
FN64="${FN64:-$(cd "$SCRIPT_DIR/.." && pwd)}"
BOOT="$FN64/examples/oot-boot"
guard="$SCRIPT_DIR/memory-guard.zsh"
export FN64_GUARD_MAX_RSS_MIB="${FN64_GUARD_MAX_RSS_MIB:-2048}"
export FN64_GUARD_MIN_FREE_PERCENT="${FN64_GUARD_MIN_FREE_PERCENT:-40}"
export CARGO_BUILD_JOBS=1
export RUST_TEST_THREADS="${RUST_TEST_THREADS:-1}"

if [ "$MODE" = selftest ]; then
  [ "$OBSERVE" -eq 0 ] && [ "$SWAPS_SET" -eq 0 ] || {
    echo "lane-parity self-test: --selftest accepts no lane options" >&2
    exit 2
  }
  [ -x "$guard" ] || { echo "lane-parity self-test: memory guard is unavailable" >&2; exit 1; }
  "$SCRIPT_DIR/native-emit.sh" --selftest >/dev/null || exit 1
  echo "lane-parity self-test: PASS"
  exit 0
fi
if [ "$MODE" = dry-run ]; then
  printf '{"schema":"fn64.lane-parity-plan.v1","status":"dry-run","observe":%s,"swaps":%s,"cargo_jobs":1,"test_threads":1,"max_rss_mib":%s,"min_free_percent":%s,"guarded_phases":["native_emit","authority_test","c_build","c_run","rs_build","rs_run"]}\n' \
    "$([ "$OBSERVE" -eq 1 ] && printf true || printf false)" "$SWAPS" \
    "$FN64_GUARD_MAX_RSS_MIB" "$FN64_GUARD_MIN_FREE_PERCENT"
  exit 0
fi

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

# --- authority gate ----------------------------------------------------------
# The rs driver may deliberately recover functions represented by callable
# empty bodies in the legacy C corpus. Framebuffer equality cannot prove those
# bodies were not reached or that their effects happened to be invisible. The
# generated-artifact test also checks that every shared nonempty function names
# the exact same unique instruction-PC set, giving an independent whole-corpus
# code-coverage differential before either boot binary runs.
echo "lane-parity: emitting/locating rs crate for authority audit..."
if [ -z "${RECOMP_RS_DIR:-}" ]; then
  if ! RECOMP_RS_DIR=$("$SCRIPT_DIR/native-emit.sh" | tail -1); then
    echo "lane-parity: FAIL -- guarded native emit failed" >&2
    exit 2
  fi
fi
if [ -z "${RECOMP_RS_DIR:-}" ] || [ ! -f "$RECOMP_RS_DIR/src/lib.rs" ]; then
  echo "lane-parity: SKIP -- no emitted rs crate (scripts/native-emit.sh produced nothing usable)." >&2
  exit 0
fi
export RECOMP_RS_DIR

AUTHORITY_MODE=require
if [ "$OBSERVE" -eq 1 ]; then
  AUTHORITY_MODE=observe
fi
echo "lane-parity: auditing generated callable bodies (mode=$AUTHORITY_MODE)..."
if ! FN64_LANE_AUTHORITY_MODE="$AUTHORITY_MODE" \
  "$guard" cargo test -j1 -q -p fn64-recomp-rs --test lane_authority \
    generated_lane_authority -- --ignored --nocapture --test-threads=1; then
  echo "lane-parity: AUTHORITY REJECTED -- the legacy C callable-body set is not aligned with the rs lane." >&2
  echo "lane-parity: use --observe only for a labeled framebuffer observation; it is not semantic parity." >&2
  exit 2
fi
if [ "$OBSERVE" -eq 1 ]; then
  echo "lane-parity: NON-AUTHORITATIVE OBSERVATION -- missing C bodies are admitted explicitly." >&2
fi

# Run one lane to $SWAPS and record "swap<TAB>sha" for every dumped framebuffer.
# The harness only dumps NON-UNIFORM framebuffers, so a blank frame is absent
# from both sides identically -- compare the dumped sets, and their equality.
run_lane() {
  local lane="$1" out="$2" bin="$3"
  [ -x "$bin" ] || { echo "lane-parity: $lane lane binary missing at $bin" >&2; exit 2; }
  rm -f "$FB_GLOB"-*.png
  ( cd "$BOOT" && OOT_MAX_SWAPS="$SWAPS" "$guard" "$bin" ) \
    >"/tmp/lane-parity-$lane.log" 2>&1
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
( cd "$BOOT" && CARGO_TARGET_DIR="$C_TARGET" FN64_RECOMP=c \
  "$guard" cargo build -j1 --release -q ) \
  || { echo "lane-parity: C lane build failed" >&2; exit 2; }
run_lane c /tmp/lane-parity-c.sha "$C_BIN"

# --- rs lane -----------------------------------------------------------------
# The emitted whole-ROM crate is content-addressed and deterministic; reuse it.
# build.rs asserts this symlink resolves to RECOMP_RS_DIR (see its rs-lane branch).
link="$BOOT/rs/recompiled"
if [ -e "$link" ] && [ ! -L "$link" ]; then
  echo "lane-parity: $link exists and is not a symlink; refusing to replace it" >&2; exit 2
fi
ln -sfn "$RECOMP_RS_DIR" "$link" || exit 2
echo "lane-parity: building rs lane (large emitted crate; first build is slow)..."
CARGO_TARGET_DIR="$RS_TARGET" FN64_RECOMP=rs \
  "$guard" cargo build -j1 --manifest-path "$BOOT/rs/Cargo.toml" --release -q \
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
  if [ "$OBSERVE" -eq 1 ]; then
    echo "lane-parity: OBSERVED MATCH -- non-authoritative C and rs framebuffers are byte-identical (<= swap $SWAPS)"
  else
    echo "lane-parity: OK -- authority-aligned C and rs framebuffers are byte-identical (<= swap $SWAPS)"
  fi
  exit 0
fi

echo "lane-parity: OBSERVED DIVERGENCE -- the two recomp lanes produced different framebuffers." >&2
first=$(join -t"$(printf '\t')" /tmp/lane-parity-c.sha /tmp/lane-parity-rs.sha \
        | awk -F'\t' '$2!=$3 {print $1; exit}')
echo "lane-parity: first divergent swap: ${first:-<set of dumped swaps differs between lanes>}" >&2
if [ -n "${first:-}" ]; then
  echo "  C : $(awk -F'\t' -v s="$first" '$1==s{print $2}' /tmp/lane-parity-c.sha)" >&2
  echo "  rs: $(awk -F'\t' -v s="$first" '$1==s{print $2}' /tmp/lane-parity-rs.sha)" >&2
fi
echo "lane-parity: full per-swap SHAs: /tmp/lane-parity-{c,rs}.sha" >&2
exit 1
