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
# gate_delta_vote: NW4E overlay VA-delta inference, graded held-out (needs
# FN64_DISCOVER_NW4E_ROM, already required above).
expected_delta_vote=4910c27983a0344115a11c8537f4a507d585ad9e3e35d8ebe108ea2fb539f4e6
# gate_gp_base: NW4E + NWXE resident $gp-base inference (both OPEN — no
# recoverable gp-small-data base in either AKI title).
expected_gp_base=d1e267035c94488b2d47df60038031ab9fc345a4daf00b68eb5b8f948abcaf6a
# gate_overlay_regions: mechanical overlay-descriptor-table discovery. Needs
# both AKI ROMs + dumps (grades held-out against the dump layout).
expected_overlay_regions=471181f20c5add3b7478e7ea65626bc8417126b33e6b9c0a5e200dd5f1cfd920
# gate_d1_overlays: NWXE boot-only versus mechanically recovered overlays.
# The dump is grading-only and opens after both discovery runs complete.
expected_d1_overlays=9b0dc15f92aac10586edf98a02873c0acfc57f4ff6f00f857546fcb1ec1c4440
# gate_d1_oot_overlays: OoT three-way A/B/C grade (boot-only vs mechanically
# recovered VROM overlays vs hand-supplied table geometry). B reaches
# 99.46%/48.45% — 67% of C's hand-geometry recall at matching precision.
# Held-out (OoT dump opens after all three discovery runs).
expected_d1_oot_overlays=ac60619581fa8b5526549929f320ebe19d84a02908ec43e2fa34dc1e2412ede9
# gate_owners_overlays: exact-owner proof on the recovered NWXE overlay banks
# (6 owners, 0 wrong extents). Dump is grading-only, opened after proof. The
# digest moved when Phase-6 indirect closure strengthened: unresolved_indirect
# occurrences fell 19196→16366 and more blocks reached; exact_owners/wrong
# unchanged (6/0).
expected_owners_overlays=ad9100231545eb7bbaab4f492531c2e5be7500b08be9778f58253424346a3717
# gate_overlay_generalize: the family search (now with VROM resolution) run
# against four NON-AKI ROMs (OoT/GE/PD/SM64). OoT now recovers 414 overlay
# regions (100% precision / 88.5% recall) via file-table VROM translation;
# SM64 stays the correct no-overlay negative control (0 admissions); GE/PD
# ungraded. Digest is fixed only with the full OoT+GE+PD+SM64 ROM set (unset
# ROMs are loud skips that change output), so it is guarded on those vars.
expected_overlay_generalize=5401e638c9c233b79ed788a824fd0666d3ad31537657f140fdf09e80fb0a9106
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

# gate_keys' recorded digest is the parse-only (no grading ROM) configuration.
# Its grading ROM vars (BANJO/PD) are SHARED with other gates, so scrub them
# for this gate specifically to keep its digest stable no matter what the
# caller set for gate_overlay_generalize etc.
check_gate_keys_parseonly() {
    expected=$1
    i=1
    while [ "$i" -le "$runs" ]; do
        got=$(env -u FN64_DISCOVER_BANJO_ROM -u FN64_DISCOVER_PD_ROM \
            cargo run --quiet --manifest-path "$repo/Cargo.toml" \
            -p fn64-discover --bin gate_keys | sha_stdout)
        if [ "$got" != "$expected" ]; then
            echo "gate_keys run $i/$runs: output sha256 $got != expected $expected" >&2
            exit 1
        fi
        i=$((i + 1))
    done
    echo "gate_keys: $runs/$runs runs byte-identical (parse-only), sha256=$expected"
}

check_gate gate_loaders "$expected_loaders"
check_gate gate_selector "$expected_selector"
check_gate gate_delta_vote "$expected_delta_vote"
# gate_keys parses the vendored answer-key tables; with the grading ROMs
# absent it is a deterministic parse-and-count run whose digest is fixed by
# the vendored table bytes (its ROM vars are scrubbed since they are shared).
check_gate_keys_parseonly "$expected_keys"

# gate_gp_base needs both AKI ROMs and their dumps (it cross-checks _gp
# symbols); guard on the NWXE dump being present.
if [ "${FN64_DISCOVER_NWXE_DUMP:-}" != "" ]; then
    check_gate gate_gp_base "$expected_gp_base"
    check_gate gate_overlay_regions "$expected_overlay_regions"
    check_gate gate_d1_overlays "$expected_d1_overlays"
    check_gate gate_owners_overlays "$expected_owners_overlays"
else
    echo "gate_gp_base: skipped (FN64_DISCOVER_NWXE_DUMP unset)"
    echo "gate_overlay_regions: skipped (FN64_DISCOVER_NWXE_DUMP unset)"
    echo "gate_d1_overlays: skipped (FN64_DISCOVER_NWXE_DUMP unset)"
    echo "gate_owners_overlays: skipped (FN64_DISCOVER_NWXE_DUMP unset)"
fi

# gate_overlay_generalize runs the AKI family search on the non-AKI corpus;
# its digest is fixed only with the full OoT+GE+PD+SM64 ROM set present.
if [ "${FN64_DISCOVER_GE_ROM:-}" != "" ] && [ "${FN64_DISCOVER_PD_ROM:-}" != "" ] \
    && [ "${FN64_DISCOVER_SM64_ROM:-}" != "" ] && [ "${FN64_DISCOVER_OOT_ROM:-}" != "" ]; then
    check_gate gate_overlay_generalize "$expected_overlay_generalize"
else
    echo "gate_overlay_generalize: skipped (needs OoT+GE+PD+SM64 ROM vars)"
fi

if [ "${FN64_DISCOVER_OOT_ROM:-}" != "" ]; then
    check_gate gate_coverage "$expected_coverage"
    check_gate gate_b2 "$expected_b2"
    if [ "${FN64_DISCOVER_OOT_DUMP:-}" != "" ]; then
        check_gate gate_d1_oot_overlays "$expected_d1_oot_overlays"
    else
        echo "gate_d1_oot_overlays: skipped (FN64_DISCOVER_OOT_DUMP unset)"
    fi
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
