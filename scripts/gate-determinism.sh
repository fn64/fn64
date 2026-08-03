#!/bin/sh
# Re-runs the deterministic fn64-discover gates and compares their complete
# stdout against the recorded SHA-256 digests cited in docs/DISCOVER-PLAN.md.
# This is the test that owns those digests (scripts/lint-docs.py rejects a
# doc-cited content hash no script or test re-checks).
#
# Inputs are named and declared, never defaulted (DESIGN.md section 1.0):
#   FN64_DISCOVER_NW4E_ROM   path to a WWF No Mercy (U) v1.1 .z64
#   FN64_DISCOVER_NWXE_ROM   path to a WWF WrestleMania 2000 (U) .z64
#   FN64_DISCOVER_OOT_ROM    path to an OoT NTSC 1.0 .z64
#   FN64_DISCOVER_NW4E_DUMP  } held-out answer keys. Optional for the script to
#   FN64_DISCOVER_NWXE_DUMP  } run, but several digests below -- notably
#   FN64_DISCOVER_OOT_DUMP   } expected_closure -- were recorded with all three
#                            } set, because a gate's stdout (and therefore its
#                            } hash) changes with which ROMs it grades. Running
#                            } with the ROMs alone yields a different but
#                            } equally deterministic digest and a false failure.
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
expected_d1_oot_overlays=c8fcb6a1fb013492cce964e71c4985ba10aa197cc0e66b9cbab57ed23493ecf7
# gate_closure: per-ROM execution-closure scoreboard (reachable destinations
# by exact_aot/block_aot/dynamic_mips/unsupported). The "unsupported" count is
# the distance to a full-game build.
#
# This digest needs ALL SIX vars, not just the three ROMs: the gate prints a
# skip line per unset ROM and grades per ROM whose DUMP is set, so its stdout
# -- and therefore this hash -- changes with any of
#   FN64_DISCOVER_{NW4E,NWXE,OOT}_{ROM,DUMP}
# The three DUMP vars are NOT required by this script's preamble, so running
# it with only the ROMs set produces a different, equally deterministic digest
# and a false failure here. Recorded under all six.
#
# Moved c8455706 -> efd25aca when OoT gained mechanical VROM overlay
# composition and the static request-DMA scan: composed banks 1 -> 923 and the
# gate grew an open_indirect_site clarification line. See the OoT rows in
# docs/DISCOVER-PLAN.md for the numbers behind the move.
#
# Moved efd25aca -> 1c6db903 for two reporting changes. NO headline count
# moved -- per_class, unsupported, and dynamic_mips are identical on all three
# ROMs; efd25aca was verified to still reproduce before the move.
#   1. `proven_code_no_owner` split out of `mapped_not_proven_code`, which had
#      been absorbing WordClass::ProvenCode through a catch-all arm. It is 96-99%
#      of that bucket (NW4E 632/650, NWXE 1756/1833, OoT 11386/11549), so the
#      old label read as "discovery could not decode this" for words discovery
#      had already proven were code.
#   2. `block_proof_blockers=` added: why block proof refused, which is the
#      actionable half of dynamic_mips.
# Retained historical schema-v1 baseline. Snapshot schema v2 intentionally
# changes block authority, so this remains a fail-loud stale-evidence sentinel
# until all six private inputs regenerate a replacement digest and docs/counts.
expected_closure=1c6db90343e63b1f482c403b1b3a057d225dc4cc9baeb6ddd5adcc4b924dc317
# gate_owners_overlays: exact-owner proof on the recovered NWXE overlay banks
# (6 owners, 0 wrong extents). Dump is grading-only, opened after proof. The
# digest moved when Phase-6 indirect closure strengthened: unresolved_indirect
# occurrences fell 19196→16366 and more blocks reached; exact_owners/wrong
# unchanged (6/0).
expected_owners_overlays=0b2f315070dbac6263f7a9d705eb162326878f79ea91f41295148761988f3a1b
# gate_corpus_homology: N-ROM mutual-labeling identity graph (6-ROM corpus, 3
# dump-graded). 635 identities, 100% held-out precision; libultra kernel
# routines span AKI+Zelda engines (OoT names propagate onto unlabeled AKI).
expected_corpus_homology=b1dd747d9cc3c214fae517db94ca8f530b6d524cd67120c865ee58fcb1e02637
# gate_callgraph_match: BinDiff MD-index call-graph propagation (NW4E<->NWXE);
# 591 body-hash seeds + 44 propagated matches, 100% precision held-out.
expected_callgraph_match=587a8cb5cc56befab6f2ba86e27d0acf829d3da17a106a16d56dc97fdb88ab89
# gate_reloc_accuracy: Decomp-Pack readiness metric (recovered references vs
# the OoT function-symbol key proxy). Held-out.
expected_reloc_accuracy=0ec62172a861da733bc5890d61e0e81075629a2986c3bf0301681e9c82c6275f
# gate_asm_roundtrip: Phase-8 assembly round trip — fn64 emits GNU-as .s from
# its OWN proven exact-owner facts, reassembles, and byte-compares to the ROM.
# 32/32 OoT boot-bank owners byte-identical. The Decomp-Pack assembly proof +
# the #25 matching-decomp prerequisite. Needs FN64_DISCOVER_OOT_ROM.
expected_asm_roundtrip=6406bfee511704589f40a624275c5ac64429e8f66e3e1d78cdc64657d32f42ca
# gate_overlay_generalize: the family search (now with VROM resolution) run
# against four NON-AKI ROMs (OoT/GE/PD/SM64). OoT now recovers 414 overlay
# regions (100% precision / 88.5% recall) via file-table VROM translation;
# SM64 stays the correct no-overlay negative control (0 admissions); GE/PD
# ungraded. Digest is fixed only with the full OoT+GE+PD+SM64 ROM set (unset
# ROMs are loud skips that change output), so it is guarded on those vars.
expected_overlay_generalize=dec5742e52cb3bcdba9ba5c7bfee25cff35e8dd2edd6aa757d17ee4d97bb9841
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
        # Output-producing closure diagnostics are outside the retained stdout
        # contract. Scrub them for every gate so an ambient opt-in cannot
        # mutate the stale-evidence sentinel or write a private artifact.
        got=$(env -u FN64_CLOSURE_AUDIT_DIR -u FN64_EMIT_BLOCK_PROGRAM \
            cargo run --quiet --manifest-path "$repo/Cargo.toml" \
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
    check_gate gate_corpus_homology "$expected_corpus_homology"
    check_gate gate_callgraph_match "$expected_callgraph_match"
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
        check_gate gate_closure "$expected_closure"
        check_gate gate_reloc_accuracy "$expected_reloc_accuracy"
        check_gate gate_asm_roundtrip "$expected_asm_roundtrip"
    else
        echo "gate_d1_oot_overlays: skipped (FN64_DISCOVER_OOT_DUMP unset)"
        echo "gate_closure: skipped (FN64_DISCOVER_OOT_DUMP unset)"
        echo "gate_reloc_accuracy: skipped (FN64_DISCOVER_OOT_DUMP unset)"
        echo "gate_asm_roundtrip: skipped (FN64_DISCOVER_OOT_DUMP unset)"
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

# --- Boundary grades: wrong == 0, the firewall -------------------------------
#
# gate_decomp_functions is checked differently from every gate above, and the
# difference is deliberate. Those gates pin a stdout digest, which is the right
# contract when any output change means a behavior change. This one grades
# recall, and recall is EXPECTED to improve: pinning its digest would turn every
# genuine recovery into a false failure and train the reader to re-baseline
# without looking.
#
# What must never move is `wrong`. A nonzero `wrong` means a discovered boundary
# split a real answer function -- the one error class this project treats as a
# regression rather than a trade (see the header of
# reference/corpus-invocations.md). So this asserts the invariant and reports
# the recall it happened to measure, rather than freezing both.
#
# Each game needs its own ROM/DUMP/donor triple, so these cannot use
# check_gate's no-env form. A game whose inputs are unset is a loud skip, never
# a silent pass.
check_boundary_grade() {
    label=$1
    shift
    # The gate itself exits nonzero on wrong>0, so its status is captured
    # rather than short-circuited: the grade line below is what says WHY, and a
    # bare "failed to run" would hide a wrong>0 behind a generic error.
    out=$(env "$@" cargo run --quiet --manifest-path "$repo/Cargo.toml" \
        -p fn64-discover --bin gate_decomp_functions 2>&1) || true
    grade=$(echo "$out" | grep -o 'matched_exact=[0-9]*.*wrong=[0-9]*' | head -1)
    case "$grade" in
        *"wrong=0")
            echo "$label: $grade" ;;
        "")
            echo "$label: gate_decomp_functions printed no grade line" >&2
            exit 1 ;;
        *)
            echo "$label: WRONG>0 -- a discovered boundary split a real answer function" >&2
            echo "  $grade" >&2
            echo "$out" | grep '^wrong:' | head -5 >&2
            exit 1 ;;
    esac
}

if [ -n "${FN64_DISCOVER_NWXE_DUMP:-}" ] && [ -n "${FN64_DISCOVER_NW4E_DUMP:-}" ]; then
    # The AKI pair donate signatures to each other: same engine, one year apart.
    check_boundary_grade "grade_nwxe" \
        "FN64_DISCOVER_ROM=$FN64_DISCOVER_NWXE_ROM" \
        "FN64_DISCOVER_DUMP=$FN64_DISCOVER_NWXE_DUMP" \
        "FN64_DISCOVER_SIG_DONOR_ROM=$FN64_DISCOVER_NW4E_ROM" \
        "FN64_DISCOVER_SIG_DONOR_DUMP=$FN64_DISCOVER_NW4E_DUMP"
    check_boundary_grade "grade_nw4e" \
        "FN64_DISCOVER_ROM=$FN64_DISCOVER_NW4E_ROM" \
        "FN64_DISCOVER_DUMP=$FN64_DISCOVER_NW4E_DUMP" \
        "FN64_DISCOVER_SIG_DONOR_ROM=$FN64_DISCOVER_NWXE_ROM" \
        "FN64_DISCOVER_SIG_DONOR_DUMP=$FN64_DISCOVER_NWXE_DUMP"
else
    echo "grade_nwxe: skipped (FN64_DISCOVER_NWXE_DUMP/NW4E_DUMP unset)"
    echo "grade_nw4e: skipped (FN64_DISCOVER_NWXE_DUMP/NW4E_DUMP unset)"
fi

# Revenge is graded WITHOUT a donor, and donor-free is the point: it is a
# generation older than the AKI late trio (8-word shingle Jaccard 0.032 against
# No Mercy, versus 0.063 between No Mercy and WM2000), and a cross-generation
# donor produces two false splits at real internal boundaries. It is graded at
# all because it is the only ROM that witnesses sig_scan::admissible_entry_word
# -- inert on every other graded game, but disabling it takes Revenge to
# wrong=4. See reference/corpus-invocations.md.
if [ -n "${FN64_DISCOVER_REVENGE_ROM:-}" ] && [ -n "${FN64_DISCOVER_REVENGE_DUMP:-}" ]; then
    check_boundary_grade "grade_revenge" \
        "FN64_DISCOVER_ROM=$FN64_DISCOVER_REVENGE_ROM" \
        "FN64_DISCOVER_DUMP=$FN64_DISCOVER_REVENGE_DUMP"
else
    echo "grade_revenge: skipped (FN64_DISCOVER_REVENGE_ROM/_DUMP unset)"
fi

echo "gate-determinism: all gates stable over $runs runs"
