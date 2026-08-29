#!/bin/zsh
# Play WM2000 in a WINDOW, with a gamepad, on fn64's all-Rust stack:
# `fn64-cpu-runtime` (FN64_RECOMP=rs) driving `fn64-render-wgpu`
# (FN64_RENDER=wgpu). No N64Recomp C bodies, no RT64 C++ adapter, and
# therefore NO `--features rt64`.
#
#   ./scripts/play-wm2000.sh
#
# That is the whole command. Everything below is defaults you can override:
#
#   FN64_RENDER=reference ./scripts/play-wm2000.sh   # the software oracle
#   FN64_SKIP_EMIT=1      ./scripts/play-wm2000.sh   # reuse the emitted crate
#   FN64_SKIP_SHELL_BUILD=1 FN64_EXPECT_SHELL_SHA256=... \
#                            ./scripts/play-wm2000.sh # reuse one exact shell
#   SCRATCH=/tmp/mine     ./scripts/play-wm2000.sh   # your own scratch root
#
# In the window: F1 settings (incl. gamepad rebinding) - F2 screenshot PNG -
# F3 stack/fps HUD - F11 fullscreen - Esc exit. A gamepad is picked up by
# HOTPLUG, so it can be connected before or after launch; the first pad you
# press a button on becomes the active one. Keyboard works with no pad at all.
#
# ---------------------------------------------------------------------------
# The two traps this script exists to encode
# ---------------------------------------------------------------------------
#
# 1. LEXICAL PATH COLLISION. `recompile_rom` writes an ABSOLUTE
#    fn64-cpu-runtime path into the crate it emits, while the shell's own rs
#    manifest names fn64 by a RELATIVE path. Cargo compares those two strings
#    LEXICALLY, not by realpath, so if they do not resolve to the same STRING
#    the build dies with:
#
#      error: package collision in the lockfile: packages fn64-cpu-runtime
#      v0.0.0 (...) and fn64-cpu-runtime v0.0.0 (...) are different
#
#    A symlink alias of the same directory does NOT fix this -- that was tried
#    and it fails identically. What fixes it is rewriting the emitted
#    manifest's path to the SAME REAL path this repo is checked out at, which
#    is what the sed below does.
#
# 2. FN64_ABSENT_N64DD=1 IS NOT OPTIONAL. It is part of osDriveRomInit's
#    disposition; without it the 64DD probe read is a loud trap BY DESIGN.
set -euo pipefail

INVOCATION_DIR=${PWD:A}

# This repo, resolved to a REAL path (see trap 1 -- a symlinked invocation
# would otherwise write a non-matching string into the emitted manifest).
FN64=${FN64:-$(cd -- "$(dirname -- "$0")/.." && pwd -P)}
AKI=${AKI:-$HOME/Code/aki-recomp}
SCRATCH=${SCRATCH:-/private/tmp/fn64-play-scratch}
EMIT=$SCRATCH/emit1
ROM=${ROM:-$AKI/games/NWXE/wm2000.z64}
# The rs lane binds host functions BY ADDRESS, so this table is per-title
# game-profile data. There is deliberately no default in build.rs: another
# title's table would resolve silently and produce wrong behaviour.
HOST_LOOKUP=${RECOMP_RS_HOST_LOOKUP:-$HOME/Code/recomps/wm2000/packages/wm2000-boot/src/host_lookup.rs}
RENDER=${FN64_RENDER:-wgpu}
APP_TITLE=${FN64_APP_TITLE:-WrestleMania 2000 [built with fn64]}

# Cargo accepts an ambient target override, so the binary selected for launch
# must be derived from the same resolved directory passed to the build. A
# fixed repo-local path can otherwise survive as an executable and be launched
# immediately after Cargo successfully built the requested revision elsewhere.
if [[ -n "${CARGO_TARGET_DIR:-}" ]]; then
  if [[ "$CARGO_TARGET_DIR" == /* ]]; then
    CARGO_TARGET_ROOT=${CARGO_TARGET_DIR:A}
  else
    CARGO_TARGET_ROOT=${INVOCATION_DIR}/${CARGO_TARGET_DIR}
    CARGO_TARGET_ROOT=${CARGO_TARGET_ROOT:A}
  fi
  RECOMPILE_TARGET_DIR=$CARGO_TARGET_ROOT
  SHELL_TARGET_DIR=$CARGO_TARGET_ROOT
else
  RECOMPILE_TARGET_DIR=$FN64/target
  SHELL_TARGET_DIR=$FN64/crates/fn64-shell/rs/target
fi
BIN=$RECOMPILE_TARGET_DIR/release/recompile_rom
SHELL_BIN=$SHELL_TARGET_DIR/release/fn64

sha256_file() {
  shasum -a 256 "$1" | awk '{print $1}'
}

verify_reused_shell() {
  [[ -f "$SHELL_BIN" && ! -L "$SHELL_BIN" && -x "$SHELL_BIN" ]] || {
    echo "[play-wm2000] FATAL: reusable shell is not a regular, non-symlink executable: $SHELL_BIN" >&2
    return 1
  }
  [[ -n "${FN64_EXPECT_SHELL_SHA256:-}" ]] || {
    echo "[play-wm2000] FATAL: FN64_SKIP_SHELL_BUILD=1 requires FN64_EXPECT_SHELL_SHA256" >&2
    return 1
  }
  [[ "$FN64_EXPECT_SHELL_SHA256" != *[^0-9a-f]* && ${#FN64_EXPECT_SHELL_SHA256} -eq 64 ]] || {
    echo "[play-wm2000] FATAL: FN64_EXPECT_SHELL_SHA256 must be exactly 64 lowercase hexadecimal characters" >&2
    return 1
  }
  local actual
  actual=$(sha256_file "$SHELL_BIN")
  [[ "$actual" == "$FN64_EXPECT_SHELL_SHA256" ]] || {
    echo "[play-wm2000] FATAL: reused shell digest mismatch: expected $FN64_EXPECT_SHELL_SHA256, measured $actual" >&2
    return 1
  }
  REUSED_SHELL_SHA256=$actual
}

if [[ "${1:-}" == --print-artifact-paths ]]; then
  (( $# == 1 )) || { echo "[play-wm2000] FATAL: --print-artifact-paths accepts no other arguments" >&2; exit 2; }
  printf 'recompile_rom=%s\nshell=%s\n' "$BIN" "$SHELL_BIN"
  exit 0
fi

# Content-free preflight for the regression suite and for operators deciding
# whether an existing shell is safe to reuse. It exercises the production
# verifier but deliberately stops before ROM or host-profile intake.
if [[ "${1:-}" == --check-shell-reuse ]]; then
  (( $# == 1 )) || { echo "[play-wm2000] FATAL: --check-shell-reuse accepts no other arguments" >&2; exit 2; }
  [[ -n "${FN64_SKIP_SHELL_BUILD:-}" ]] || {
    echo "[play-wm2000] FATAL: --check-shell-reuse requires FN64_SKIP_SHELL_BUILD=1" >&2
    exit 2
  }
  verify_reused_shell
  echo "[play-wm2000] reusable shell verified: $SHELL_BIN (sha256=$REUSED_SHELL_SHA256)"
  exit 0
fi

for f in "$ROM" "$HOST_LOOKUP"; do
  [[ -f "$f" ]] || { echo "[play-wm2000] FATAL: missing $f" >&2; exit 1; }
done

# 1. Emit the whole-ROM Rust crate. Guarded by the same staleness rule
#    run-rs-lane.sh uses: a `recompile_rom` older than the codegen sources it
#    was built from silently emits a crate WITHOUT the fix under test, and the
#    run then "reproduces" a blocker that is already fixed. That cost two wrong
#    conclusions in one day.
if [[ -z "${FN64_SKIP_EMIT:-}" ]]; then
  echo "[play-wm2000] building recompile_rom (FN64_SKIP_EMIT=1 to reuse an existing emit)"
  ( cd "$FN64" && CARGO_TARGET_DIR="$RECOMPILE_TARGET_DIR" \
      cargo build --release --bin recompile_rom --offline )
  NEWER=$(find "$FN64/crates/fn64-cpu-runtime-codegen/src" "$FN64/crates/fn64-cpu-runtime/src" \
            -name '*.rs' -newer "$BIN" -print -quit 2>/dev/null || true)
  if [[ -n "$NEWER" ]]; then
    echo "[play-wm2000] FATAL: recompile_rom is STALE -- $NEWER is newer than the binary." >&2
    exit 1
  fi
  mkdir -p "$EMIT"
  "$BIN" --config "$AKI/games/NWXE/wm2000.toml" --rom "$ROM" --out "$EMIT"
else
  [[ -d "$EMIT" ]] || { echo "[play-wm2000] FATAL: FN64_SKIP_EMIT=1 but $EMIT does not exist" >&2; exit 1; }
  echo "[play-wm2000] reusing emitted crate at $EMIT"
fi

# 2. Defuse trap 1: point the emitted crate at THIS checkout's real path, and
#    bridge the emitted crate in via the symlink Cargo `path` cannot express
#    with an env var.
/usr/bin/sed -i '' \
  "s|^fn64-cpu-runtime = .*|fn64-cpu-runtime = { path = \"$FN64/crates/fn64-cpu-runtime\" }|" \
  "$EMIT/Cargo.toml"
ln -sfn "$EMIT" "$FN64/crates/fn64-shell/rs/recompiled"

# 3. Build the windowed shell on the rs lane. No `--features rt64`.
#    FN64_RENDER=wgpu needs no Cargo feature: WgpuBackend::try_new is
#    unconditionally available.
cd "$FN64/crates/fn64-shell/rs"
if [[ -z "${FN64_SKIP_SHELL_BUILD:-}" ]]; then
  echo "[play-wm2000] building the shell (rs lane, renderer=$RENDER)"
  CARGO_TARGET_DIR="$SHELL_TARGET_DIR" \
  FN64_RECOMP=rs \
  FN64_APP_TITLE="$APP_TITLE" \
  ROM="$ROM" \
  RECOMP_RS_HOST_LOOKUP="$HOST_LOOKUP" \
    cargo build --release --offline
else
  verify_reused_shell
  echo "[play-wm2000] reusing linked shell at $SHELL_BIN (sha256=$REUSED_SHELL_SHA256)"
fi

[[ -f "$SHELL_BIN" && ! -L "$SHELL_BIN" && -x "$SHELL_BIN" ]] || {
  echo "[play-wm2000] FATAL: selected shell is not a regular, non-symlink executable: $SHELL_BIN" >&2
  exit 1
}
SHELL_SHA256=$(sha256_file "$SHELL_BIN")
echo "[play-wm2000] selected shell: $SHELL_BIN (sha256=$SHELL_SHA256)"

# 4. Play. The startup banner names the lane and the RESOLVED renderer -- if
#    it says `reference-fallback`, wgpu failed to construct and the reason is
#    on the line above it. Paste that [fn64-stack] block into any report.
FINAL_SHELL_SHA256=$(sha256_file "$SHELL_BIN")
[[ "$FINAL_SHELL_SHA256" == "$SHELL_SHA256" ]] || {
  echo "[play-wm2000] FATAL: selected shell changed before launch: expected $SHELL_SHA256, measured $FINAL_SHELL_SHA256" >&2
  exit 1
}
exec env \
  ROM="$ROM" \
  FN64_ABSENT_N64DD=1 \
  FN64_RENDER="$RENDER" \
  "$SHELL_BIN" "$@"
