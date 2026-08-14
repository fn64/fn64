#!/bin/zsh
# Capture a ROM-bound post-IPL3 boot context, and put it somewhere durable.
#
# `FN64_BOOT_CONTEXT` is the authority for initial COP0 Status. It cannot be
# synthesized: the two `BootContext` builders in the tree are test fixtures
# carrying placeholder IPL3 digests, and a hand-written one would pass schema
# validation while binding register state the hardware never produced --
# forging the very authority under audit.
#
# It has to be captured from the public m64p debugger boundary, which this
# wraps. The 2026-08-03 frontier audit recorded `initial_cop0_status =
# {"authority":"missing"}` purely because no capture existed on this machine.
#
# Output goes OUTSIDE /tmp on purpose. macOS reaps /private/tmp, and that is
# how the previous session's route logs and its 420,000-step schedule were
# lost -- the run that reached WM2000's match-setup screen is no longer
# reproducible because its evidence sat in a reaped directory.
#
# Usage: scripts/capture-boot-context.zsh <rom.z64> [dest.json]
set -eu

typeset -r repo=${0:A:h:h}
typeset -r rom=${1:?usage: capture-boot-context.zsh <rom.z64> [dest.json]}
# Corpus ROM filenames carry spaces and parentheses -- "WWF No Mercy (USA)
# (Rev A).z64" -- and run-black-box-trace rejects a trace ID that is not a
# portable identifier. Slugify once and use it for both the trace ID and the
# default destination, so the script works on any corpus file rather than only
# on hand-renamed copies.
typeset -r slug=${${${rom:t:r}//[^A-Za-z0-9]/-}//---#/-}
typeset -r dest=${2:-$HOME/Code/aki-recomp/captures/${slug}-boot-context.json}

typeset -r core=/private/tmp/fn64-mupen-core-current/mupen64plus-core/projects/unix/libmupen64plus.dylib
typeset -r rsp=/private/tmp/fn64-rsp-hle-build/src/projects/unix/mupen64plus-rsp-hle.dylib
typeset -r headers=/opt/homebrew/Cellar/mupen64plus/2.6.0/include

for required in "$rom" "$core" "$rsp"; do
    [[ -r "$required" ]] || { print -u2 -- "capture-boot-context: missing $required"; exit 1 }
done

# Rebuild the producer rather than trusting a stale binary: a previously built
# mupen_trace predated edits to its own source, and the two differed by 14 KB.
typeset -r build=$(mktemp -d /private/tmp/fn64-trace-build.XXXXXX)
cc -O2 -Wall -Wextra -o "$build/mupen_trace" \
    "$repo/tools/mupen-trace/mupen_trace.c" -I"$headers" -lpthread

cargo build --quiet --release --manifest-path "$repo/Cargo.toml" \
    -p fn64-discover --bin fn64-discover

# run-black-box-trace.zsh requires a create-new directory outside the worktree.
typeset -r out=$(mktemp -d /private/tmp/fn64-capture.XXXXXX)/run
"$repo/scripts/run-black-box-trace.zsh" \
    --producer "$build/mupen_trace" \
    --discover "$repo/target/release/fn64-discover" \
    --core "$core" --rsp "$rsp" --rom "$rom" \
    --trace-id "${slug}-boot" \
    --steps 2000000 --timeout-seconds 420 --out-dir "$out"

[[ -s "$out/boot-context.json" ]] || {
    print -u2 -- "capture-boot-context: producer wrote no boot context"; exit 1 }

mkdir -p "${dest:h}"
cp "$out/boot-context.json" "$dest"
print -- "$dest"
