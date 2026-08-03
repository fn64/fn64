#!/bin/zsh

# Fast, bounded static-frontier inventory for the standalone WM2000 artifact.
# A successful inventory is not a catalog-exhaustiveness claim.

set -eu

typeset -r repo_root=${0:A:h:h}

if [[ -z ${FN64_DISCOVER_NWXE_ROM:-} ]]; then
    print -u2 -- "wm2000 static frontier: FN64_DISCOVER_NWXE_ROM must name the local normalized-ROM input"
    exit 2
fi
if [[ -z ${FN64_BOOT_CONTEXT:-} || ! -f "$FN64_BOOT_CONTEXT" ]]; then
    print -u2 -- "wm2000 static frontier: FN64_BOOT_CONTEXT must name the ROM-bound header-entry capture"
    exit 2
fi

"$repo_root/scripts/check-wm2000-pure-aot.zsh"

export FN64_DENSE_MANIFEST_ONLY=1
export FN64_GUARD_MAX_RSS_MIB=${FN64_GUARD_MAX_RSS_MIB:-2048}
# Static receipt production never debugs this host binary. Line tables retain
# actionable panic locations while avoiding the measured full-debuginfo rustc
# peak that exceeds the fixed 2 GiB safety envelope.
export CARGO_PROFILE_DEV_DEBUG=${CARGO_PROFILE_DEV_DEBUG:-1}
if [[ -n ${FN64_WM2000_FRONTIER_BIN:-} ]]; then
    typeset -r frontier_bin=$FN64_WM2000_FRONTIER_BIN
    [[ $frontier_bin == /* && ${frontier_bin:A} == $frontier_bin \
        && -f $frontier_bin && -x $frontier_bin && ! -L $frontier_bin ]] || {
        print -u2 -- "wm2000 static frontier: FN64_WM2000_FRONTIER_BIN must name a canonical absolute executable regular file"
        exit 2
    }
    "$repo_root/scripts/memory-guard.zsh" "$frontier_bin"
else
    "$repo_root/scripts/memory-guard.zsh" \
        cargo run -q -j1 -p fn64-discover --bin gate_wm2000_recompile
fi
