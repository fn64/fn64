#!/bin/sh
# Re-runs the deterministic fn64-discover gates and compares their complete
# stdout against the recorded SHA-256 digests cited in docs/DISCOVER-PLAN.md.
# This is the test that owns those digests (scripts/lint-docs.py rejects a
# doc-cited content hash no script or test re-checks).
#
# Inputs are named and declared, never defaulted (DESIGN.md section 1.0):
#   FN64_DISCOVER_NW4E_ROM   path to a WWF No Mercy (U) v1.1 .z64
#   FN64_DISCOVER_NWXE_ROM   path to a WWF WrestleMania 2000 (U) .z64
#   FN64_DISCOVER_OOT_ROM    path to an OoT NTSC 1.0 .z64 (gate_coverage only)
#
# Usage: scripts/gate-determinism.sh [runs]   (default 10)
set -eu

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
runs=${1:-10}

: "${FN64_DISCOVER_NW4E_ROM:?set FN64_DISCOVER_NW4E_ROM to the NW4E .z64 path}"
: "${FN64_DISCOVER_NWXE_ROM:?set FN64_DISCOVER_NWXE_ROM to the NWXE .z64 path}"

# Expected full-stdout digests. A change in gate output is a real behavior
# change: update the digest here and the citation in docs/DISCOVER-PLAN.md in
# the same commit, with the evidence for why the output moved.
expected_loaders=5a67f5e471bad44bbb85aba27decd4ac831d93f2a24f0de1b329c3393bfec921
expected_selector=b53b25c7dd0a92dda59182f78f5c3ac0e0147124ea19941516da92a391679290
# gate_coverage renders the metric ladder for every supplied ROM; its digest is
# only fixed when all three ROM vars are set, since an unset var is a loud skip
# line that legitimately changes the output.
expected_coverage=6153e54d4f04af85645795c5e2a5a2192391b4eeb6978dd2d88b44aaedcd07c6

sha_stdout() {
    shasum -a 256 | awk '{print $1}'
}

check_gate() {
    gate=$1
    expected=$2
    i=1
    while [ "$i" -le "$runs" ]; do
        got=$(cargo run --quiet --manifest-path "$repo/Cargo.toml" \
            -p fn64-discover --bin "$gate" | sha_stdout)
        if [ "$got" != "$expected" ]; then
            echo "$gate run $i/$runs: output sha256 $got != expected $expected" >&2
            exit 1
        fi
        i=$((i + 1))
    done
    echo "$gate: $runs/$runs runs byte-identical, sha256=$expected"
}

check_gate gate_loaders "$expected_loaders"
check_gate gate_selector "$expected_selector"

if [ "${FN64_DISCOVER_OOT_ROM:-}" != "" ]; then
    check_gate gate_coverage "$expected_coverage"
else
    echo "gate_coverage: skipped (FN64_DISCOVER_OOT_ROM unset; digest not checkable)"
fi

echo "gate-determinism: all gates stable over $runs runs"
