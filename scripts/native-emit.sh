#!/usr/bin/env bash
# Emit-cache for the whole-ROM Rust-recompiled module. `recompile_rom` is DETERMINISTIC
# (same ROM + config + recompiler => bit-identical 139MB funcs crate), so every
# job re-emitting it is pure waste (~10s + 133MB each). This caches the emit by
# hash(ROM + oot.toml + recompile_rom binary) and reuses it.
#
#   native-emit.sh           -> prints the cache dir (emits only on a miss)
#   native-emit.sh --force   -> re-emit even on a hit
#   native-emit.sh --dry-run -> print the path-free guarded phase plan
#   native-emit.sh --selftest
#
# Env: FN64_CONFIG (default aki-recomp OOTU oot.toml), FN64_ROM (from the config),
# FN64_ROOT (default: repo root). Cache lives in $FN64_EMIT_CACHE (default
# /tmp/fn64-emit-cache/<hash>).
# ponytail: a content-addressed cache, nothing fancier. No eviction policy —
# add one if /tmp fills; a hash dir is a few hundred MB and rarely churns.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
FN64_ROOT="$(pwd)"
guard="$FN64_ROOT/scripts/memory-guard.zsh"
export FN64_GUARD_MAX_RSS_MIB="${FN64_GUARD_MAX_RSS_MIB:-2048}"
export FN64_GUARD_MIN_FREE_PERCENT="${FN64_GUARD_MIN_FREE_PERCENT:-40}"
export CARGO_BUILD_JOBS=1

mode=run
force=0
case "${1:-}" in
  "") ;;
  --force) force=1 ;;
  --dry-run) mode=dry-run ;;
  --selftest) mode=selftest ;;
  *) echo "usage: scripts/native-emit.sh [--force|--dry-run|--selftest]" >&2; exit 2 ;;
esac
if [ "$#" -gt 1 ]; then
  echo "usage: scripts/native-emit.sh [--force|--dry-run|--selftest]" >&2
  exit 2
fi
if [ "$mode" = selftest ]; then
  [ -x "$guard" ] || { echo "native-emit self-test: memory guard is unavailable" >&2; exit 1; }
  echo "native-emit self-test: PASS"
  exit 0
fi
if [ "$mode" = dry-run ]; then
  printf '{"schema":"fn64.native-emit-plan.v1","status":"dry-run","cargo_jobs":1,"max_rss_mib":%s,"min_free_percent":%s,"guarded_phases":["driver_build","rom_emit"]}\n' \
    "$FN64_GUARD_MAX_RSS_MIB" "$FN64_GUARD_MIN_FREE_PERCENT"
  exit 0
fi
# FN64_GAME_DIR: the workspace holding YOUR ROM-derived material. No default --
# a path baked in here works for one machine and fails confusingly elsewhere.
# An unset var just means "use the default", so a silent rename would let a
# stale OOT_CONFIG=... point at one config while we emit from another.
for legacy in OOT_CONFIG OOT_ROM OOT_OUT; do
  if [ -n "${!legacy:-}" ]; then
    echo "native-emit: $legacy was renamed to FN64_${legacy#OOT_}; unset $legacy and set FN64_${legacy#OOT_} instead" >&2
    exit 2
  fi
done

FN64_CONFIG="${FN64_CONFIG:-${FN64_GAME_DIR:?set FN64_GAME_DIR (your ROM-derived workspace), or set FN64_CONFIG and FN64_ROM directly}/games/OOTU/oot.toml}"
FN64_ROM="${FN64_ROM:-${FN64_GAME_DIR:?set FN64_GAME_DIR, or set FN64_ROM directly}/games/OOTU/oot-ntsc-1.0.z64}"
CACHE_ROOT="${FN64_EMIT_CACHE:-/tmp/fn64-emit-cache}"

[ -f "$FN64_CONFIG" ] || { echo "no config: $FN64_CONFIG" >&2; exit 2; }
[ -f "$FN64_ROM" ]    || { echo "no ROM: $FN64_ROM" >&2; exit 2; }

# Build the driver once (cheap, cached by cargo) and hash it into the key so a
# recompiler change invalidates the cache.
"$guard" cargo build -j1 --release -q -p fn64-cpu-runtime-codegen --bin recompile_rom \
  >/dev/null || { echo "native-emit: guarded recompile_rom build failed" >&2; exit 3; }
# Ask cargo where it put the binary rather than assuming repo-local target/:
# FAST-LOOP.md tells every rs-lane job to export CARGO_TARGET_DIR, which moves
# it. Guessing $FN64_ROOT/target silently hashed a STALE driver into the cache
# key when both copies existed, and failed outright on a fresh clone.
DRIVER="${CARGO_TARGET_DIR:-$FN64_ROOT/target}/release/recompile_rom"
[ -x "$DRIVER" ] || { echo "native-emit: driver not at $DRIVER (cargo build failed?)" >&2; exit 3; }

key=$( { md5 -q "$FN64_ROM"; md5 -q "$FN64_CONFIG"; md5 -q "$DRIVER"; } | md5 -q )
OUT="$CACHE_ROOT/$key"

if [ "$force" -eq 0 ] && [ -f "$OUT/Cargo.toml" ] && [ -f "$OUT/src/lib.rs" ]; then
  echo "$OUT"   # cache HIT — reuse the deterministic emit
  echo "recomp-rs-emit: cache HIT $key" >&2
  exit 0
fi

echo "recomp-rs-emit: cache MISS $key -- emitting..." >&2
mkdir -p "$OUT"
"$guard" "$DRIVER" --config "$FN64_CONFIG" --rom "$FN64_ROM" --out "$OUT" >&2 \
  || { echo "native-emit: guarded ROM emission failed" >&2; exit 3; }
echo "$OUT"
