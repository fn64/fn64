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
# gate_keys in the ROMs-absent (parse-only) configuration.
expected_keys=d3c0dfbeb85b3042b55f4d896d69c111d5e8d7c0ec3fe513ac7dfd679903af71
# gate_coverage renders the metric ladder for every supplied ROM; its digest is
# only fixed when all three ROM vars are set, since an unset var is a loud skip
# line that legitimately changes the output.
expected_coverage=6153e54d4f04af85645795c5e2a5a2192391b4eeb6978dd2d88b44aaedcd07c6
# gate_b2's stdout includes the NWXE BlockPack sha256 and the owner-admission
# and runner-harness lines, so checking this digest re-checks the pack
# identity too. Requires all three ROM vars (H3: env-driven since 2026-07-18;
# its answer keys live in testdata/). The pack digest is also asserted
# directly below so a citation of it stays test-owned even if the stdout
# digest is legitimately updated.
expected_b2=b047ad77d16bed03384508520788786978faba2b3296a73315ce01554388f099
expected_nwxe_pack=5944f1a0c63523591cbef33c4856c594b2cca38466945bc63da35a7459dace44

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
# gate_keys parses the vendored answer-key tables; with the grading ROMs
# absent (FN64_DISCOVER_BANJO_ROM/PD_ROM unset) it is a deterministic
# parse-and-count run whose digest is fixed by the vendored table bytes.
check_gate gate_keys "$expected_keys"

if [ "${FN64_DISCOVER_OOT_ROM:-}" != "" ]; then
    check_gate gate_coverage "$expected_coverage"
    check_gate gate_b2 "$expected_b2"
    b2_out=$(cargo run --quiet --manifest-path "$repo/Cargo.toml" -p fn64-discover --bin gate_b2)
    case "$b2_out" in
        *"sha256=$expected_nwxe_pack"*)
            echo "gate_b2: NWXE pack digest $expected_nwxe_pack confirmed" ;;
        *)
            echo "gate_b2 no longer reports NWXE pack sha256=$expected_nwxe_pack" >&2
            exit 1 ;;
    esac
else
    echo "gate_coverage: skipped (FN64_DISCOVER_OOT_ROM unset; digest not checkable)"
    echo "gate_b2: skipped (FN64_DISCOVER_OOT_ROM unset; digest not checkable)"
fi

echo "gate-determinism: all gates stable over $runs runs"
