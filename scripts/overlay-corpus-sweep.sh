#!/usr/bin/env bash
# Measure overlay-table recovery across the whole ROM corpus.
#
# M1b lowered the descriptor-record floor to 2 and taught the swapping-engine
# recognizer to read a contiguous two-source pair as a reused slot. Both rules
# are general, so the question this answers is whether the win is AKI-specific
# or corpus-wide: how many ROMs recover overlay geometry now that did not
# before, and how many regress.
#
# Pure measurement -- no repository state changes. Writes TSV to stdout.
#
# Usage: scripts/overlay-corpus-sweep.sh [jobs]
set -uo pipefail
cd "$(dirname "$0")/.."
[ -f .claude/local.env ] && source .claude/local.env
jobs=${1:-6}
probe=./target/release/examples/probe_overlay_min_records
[ -x "$probe" ] || {
    echo "build first: cargo build --release -p fn64-discover --example probe_overlay_min_records" >&2
    exit 1
}

measure() {
    rom=$1
    name=$(basename "$rom" .z64)
    # min_records=3 reproduces the pre-M1b floor; 2 is current HEAD.
    before=$("$probe" "$rom" 3 2>/dev/null | head -1)
    after=$("$probe" "$rom" 2 2>/dev/null | head -1)
    ba=$(printf '%s' "$before" | grep -o 'admitted_tables=[0-9]*' | cut -d= -f2)
    bi=$(printf '%s' "$before" | grep -o 'admitted_intervals=[0-9]*' | cut -d= -f2)
    aa=$(printf '%s' "$after" | grep -o 'admitted_tables=[0-9]*' | cut -d= -f2)
    ai=$(printf '%s' "$after" | grep -o 'admitted_intervals=[0-9]*' | cut -d= -f2)
    printf '%s\t%s\t%s\t%s\t%s\n' "$name" "${ba:-ERR}" "${bi:-ERR}" "${aa:-ERR}" "${ai:-ERR}"
}
export -f measure
export probe

printf 'rom\ttables_before\tintervals_before\ttables_after\tintervals_after\n'
ls "$FN64_ROM_CORPUS_DIR"/*.z64 | xargs -P "$jobs" -I{} bash -c 'measure "$@"' _ {}
