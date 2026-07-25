#!/usr/bin/env bash
# Build the DEBUGGER=1 mupen64plus-core that tools/mupen-trace/*.c dlopen at
# runtime, from a pinned source revision.
#
# WHY A PINNED FORK: upstream mupen64plus-core does not build a working
# DEBUGGER=1 core on macOS/arm64. Two patches are required, both of which are
# in flight upstream as PR #1184:
#   3af70b7c  new_dynarec: native Apple Silicon (darwin-arm64) support
#   c6cf52d5  debugger: build DEBUGGER=1 on macOS/arm64 without libbfd/libopcodes
# When #1184 merges, repoint CORE_REPO at upstream and CORE_COMMIT at the merge,
# and delete this comment.
#
# LICENSING: mupen64plus-core is GPLv2. fn64 NEVER links it. The producers in
# this directory dlopen the built dylib at runtime through the public m64p
# frontend API and run as a separate process; no GPL source is vendored into
# fn64's tree and no GPL symbol is link-time bound. Keep it that way -- the
# build output belongs in an out-of-tree scratch dir, never in the git tree.
#
# Usage: tools/mupen-trace/build-core.sh <out-of-tree-scratch-dir>
# Output: <scratch>/mupen64plus-core/projects/unix/libmupen64plus.dylib (or .so)

set -euo pipefail

CORE_REPO="https://github.com/jeremyw/mupen64plus-core.git"
# Tip of jeremyw:darwin-arm64-dynarec == PR #1184 head. Carries both patches.
CORE_COMMIT="c6cf52d517e63fe4bed01554ddfbd9af5fb48d5a"
# The upstream commit those two patches sit on, recorded so drift is visible.
CORE_UPSTREAM_BASE="6dca4c15370ac3e2171ce7b31426695f8f39b460"

OUT_DIR="${1:?usage: build-core.sh <out-of-tree-scratch-dir>}"
mkdir -p "$OUT_DIR"
SRC="$OUT_DIR/mupen64plus-core"

if [ -d "$SRC/.git" ]; then
  echo "Reusing existing clone at $SRC" >&2
  git -C "$SRC" fetch --quiet origin "$CORE_COMMIT" 2>/dev/null || true
else
  echo "Cloning $CORE_REPO ..." >&2
  git clone --quiet "$CORE_REPO" "$SRC"
fi

git -C "$SRC" checkout --quiet --detach "$CORE_COMMIT"
GOT=$(git -C "$SRC" rev-parse HEAD)
if [ "$GOT" != "$CORE_COMMIT" ]; then
  echo "error: checked out $GOT, expected $CORE_COMMIT" >&2
  exit 1
fi
echo "Pinned core at $CORE_COMMIT (upstream base $CORE_UPSTREAM_BASE)" >&2

# DEBUGGER=1 is the whole point: it exports DebugSetCallbacks / DebugStep /
# DebugMemRead32, which the producers here drive. On macOS the second pinned
# patch defaults DEBUGGER_NO_DISASM=1, so no libbfd/libopcodes is needed.
JOBS=$( (command -v nproc >/dev/null && nproc) || sysctl -n hw.ncpu || echo 4)
echo "Building DEBUGGER=1 core with -j$JOBS ..." >&2
make -C "$SRC/projects/unix" all DEBUGGER=1 "-j$JOBS" >"$OUT_DIR/build.log" 2>&1 || {
  echo "error: build failed; see $OUT_DIR/build.log" >&2
  tail -30 "$OUT_DIR/build.log" >&2
  exit 1
}

DYLIB=$(ls "$SRC/projects/unix"/libmupen64plus.dylib \
           "$SRC/projects/unix"/libmupen64plus.so.* 2>/dev/null | head -1 || true)
if [ -z "$DYLIB" ]; then
  echo "error: no libmupen64plus shared object was produced" >&2
  exit 1
fi

# A core built WITHOUT DEBUGGER=1 links and loads fine but silently lacks these
# symbols, and the producers then fail at dlsym with a confusing message. Verify
# here instead, where the cause is obvious.
MISSING=""
for sym in DebugSetCallbacks DebugStep DebugMemRead32; do
  if ! nm -gU "$DYLIB" 2>/dev/null | grep -q "_${sym}$"; then
    MISSING="$MISSING $sym"
  fi
done
if [ -n "$MISSING" ]; then
  echo "error: built core is missing debugger symbol(s):$MISSING" >&2
  echo "       (was DEBUGGER=1 actually applied?)" >&2
  exit 1
fi

echo "OK: DEBUGGER=1 core with debugger symbols at" >&2
echo "$DYLIB"
