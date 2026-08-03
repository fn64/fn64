#!/usr/bin/env bash
# Grade every firewall configuration in parallel and print one table.
#
# The serial form was the slow part of the mechanism loop: five independent
# cargo runs, each a couple of minutes, executed one after another. They share
# no state, so they run concurrently against one pre-built binary.
#
# Usage: scripts/grade-all.sh [baseline.tsv]
#   With an argument, diffs against that file and flags any regression.
#   Writes the current table to /tmp/fn64-grades.tsv for the next comparison.
set -uo pipefail
cd "$(dirname "$0")/.."
[ -f .claude/local.env ] && source .claude/local.env

REV_ROM="${FN64_DISCOVER_REVENGE_ROM:-$HOME/Code/aki-recomp/donors/wcw-nwo-revenge.z64}"
REV_DUMP="${FN64_DISCOVER_REVENGE_DUMP:-$HOME/Code/aki-recomp/refs/WCWnWoRevengeRecomp/syms/dump.toml}"

# Build once; the parallel runs then only execute.
cargo build --quiet --release -p fn64-discover --bin gate_decomp_functions || exit 1
GATE=./target/release/gate_decomp_functions

run_one() {
    label=$1; shift
    line=$(env "$@" "$GATE" 2>&1 | grep -o 'matched_exact=[0-9]*.*wrong=[0-9]*' | head -1)
    exact=$(printf '%s' "$line" | grep -o 'matched_exact=[0-9]*' | cut -d= -f2)
    wrong=$(printf '%s' "$line" | grep -o 'wrong=[0-9]*' | cut -d= -f2)
    printf '%s\t%s\t%s\n' "$label" "${exact:-ERR}" "${wrong:-ERR}"
}

{
    run_one nwxe-solo \
        "FN64_DISCOVER_ROM=$FN64_DISCOVER_NWXE_ROM" \
        "FN64_DISCOVER_DUMP=$FN64_DISCOVER_NWXE_DUMP" &
    run_one nwxe-donor \
        "FN64_DISCOVER_ROM=$FN64_DISCOVER_NWXE_ROM" \
        "FN64_DISCOVER_DUMP=$FN64_DISCOVER_NWXE_DUMP" \
        "FN64_DISCOVER_SIG_DONOR_ROM=$FN64_DISCOVER_NW4E_ROM" \
        "FN64_DISCOVER_SIG_DONOR_DUMP=$FN64_DISCOVER_NW4E_DUMP" &
    run_one nw4e-solo \
        "FN64_DISCOVER_ROM=$FN64_DISCOVER_NW4E_ROM" \
        "FN64_DISCOVER_DUMP=$FN64_DISCOVER_NW4E_DUMP" &
    run_one nw4e-donor \
        "FN64_DISCOVER_ROM=$FN64_DISCOVER_NW4E_ROM" \
        "FN64_DISCOVER_DUMP=$FN64_DISCOVER_NW4E_DUMP" \
        "FN64_DISCOVER_SIG_DONOR_ROM=$FN64_DISCOVER_NWXE_ROM" \
        "FN64_DISCOVER_SIG_DONOR_DUMP=$FN64_DISCOVER_NWXE_DUMP" &
    run_one revenge-solo \
        "FN64_DISCOVER_ROM=$REV_ROM" \
        "FN64_DISCOVER_DUMP=$REV_DUMP" &
    wait
} | sort > /tmp/fn64-grades.tsv

baseline=${1:-}
printf '%-14s %8s %6s' config exact wrong
[ -n "$baseline" ] && printf '  %8s' delta
printf '\n'
status=0
while IFS=$'\t' read -r label exact wrong; do
    printf '%-14s %8s %6s' "$label" "$exact" "$wrong"
    if [ -n "$baseline" ] && [ -f "$baseline" ]; then
        was=$(grep -P "^$label\t" "$baseline" 2>/dev/null | cut -f2)
        if [ -n "$was" ] && [ "$exact" != ERR ]; then
            printf '  %+8d' "$((exact - was))"
            [ "$exact" -lt "$was" ] && printf ' REGRESSION' && status=1
        fi
    fi
    # wrong>0 is disqualifying in every configuration -- see the
    # fn64-firewall skill.
    [ "$wrong" != 0 ] && printf '  *** WRONG>0 ***' && status=1
    printf '\n'
done < /tmp/fn64-grades.tsv
exit $status
