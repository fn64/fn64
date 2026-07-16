#!/usr/bin/env bash
# Emit-cache for the whole-ROM Rust-recompiled module. `recompile_rom` is DETERMINISTIC
# (same ROM + config + recompiler => bit-identical 139MB funcs crate), so every
# job re-emitting it is pure waste (~10s + 133MB each). This caches the emit by
# hash(ROM + oot.toml + recompile_rom binary) and reuses it.
#
#   native-emit.sh          -> prints the cache dir (emits only on a miss)
#   native-emit.sh --force  -> re-emit even on a hit
#
# Env: OOT_CONFIG (default aki-recomp OOTU oot.toml), OOT_ROM (from the config),
# FN64_ROOT (default: repo root). Cache lives in $FN64_EMIT_CACHE (default
# /tmp/fn64-emit-cache/<hash>).
# ponytail: a content-addressed cache, nothing fancier. No eviction policy —
# add one if /tmp fills; a hash dir is a few hundred MB and rarely churns.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
FN64_ROOT="$(pwd)"
AKI="${AKI:-/Users/jer/Code/aki-recomp}"
OOT_CONFIG="${OOT_CONFIG:-$AKI/games/OOTU/oot.toml}"
OOT_ROM="${OOT_ROM:-$AKI/games/OOTU/oot-ntsc-1.0.z64}"
CACHE_ROOT="${FN64_EMIT_CACHE:-/tmp/fn64-emit-cache}"

[ -f "$OOT_CONFIG" ] || { echo "no config: $OOT_CONFIG" >&2; exit 2; }
[ -f "$OOT_ROM" ]    || { echo "no ROM: $OOT_ROM" >&2; exit 2; }

# Build the driver once (cheap, cached by cargo) and hash it into the key so a
# recompiler change invalidates the cache.
cargo build --release -q -p fn64-recomp-rs --bin recompile_rom >/dev/null 2>&1
DRIVER="$FN64_ROOT/target/release/recompile_rom"

key=$( { md5 -q "$OOT_ROM"; md5 -q "$OOT_CONFIG"; md5 -q "$DRIVER"; } | md5 -q )
OUT="$CACHE_ROOT/$key"

if [ "${1:-}" != "--force" ] && [ -f "$OUT/Cargo.toml" ] && [ -f "$OUT/src/lib.rs" ]; then
  echo "$OUT"   # cache HIT — reuse the deterministic emit
  echo "recomp-rs-emit: cache HIT $key" >&2
  exit 0
fi

echo "recomp-rs-emit: cache MISS $key -- emitting..." >&2
mkdir -p "$OUT"
"$DRIVER" --config "$OOT_CONFIG" --rom "$OOT_ROM" --out "$OUT" >&2
echo "$OUT"
