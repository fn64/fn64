#!/usr/bin/env python3
"""Measure overlay-table recovery across the whole ROM corpus.

M1b lowered the descriptor-record floor to 2 and taught the swapping-engine
recognizer to read a contiguous two-source pair as a reused slot. Both rules
are general, so the question this answers is whether the win is AKI-specific
or corpus-wide: which ROMs recover overlay geometry at HEAD that did not at
the previous floor, and whether any regress.

Pure measurement -- reads ROMs, writes a TSV, changes no repository state.
The shell version of this sweep silently measured 8 of 287 ROMs because
`export -f` + `xargs bash -c` does not survive every shell; a process pool
here removes that whole class of failure.
"""

import argparse
import concurrent.futures
import glob
import os
import re
import subprocess
import sys

PROBE = "./target/release/examples/probe_overlay_min_records"
# min_records=3 reproduces the pre-M1b floor; 2 is current HEAD.
FLOOR_BEFORE, FLOOR_AFTER = "3", "2"


def counts(rom, floor):
    """(admitted_tables, admitted_intervals) for one ROM at one floor."""
    try:
        out = subprocess.run(
            [PROBE, rom, floor], capture_output=True, text=True, timeout=600
        ).stdout
    except subprocess.TimeoutExpired:
        return None
    tables = re.search(r"admitted_tables=(\d+)", out)
    intervals = re.search(r"admitted_intervals=(\d+)", out)
    if not tables or not intervals:
        return None
    return int(tables.group(1)), int(intervals.group(1))


def measure(rom):
    name = os.path.basename(rom)[:-4]
    before = counts(rom, FLOOR_BEFORE)
    after = counts(rom, FLOOR_AFTER)
    if before is None or after is None:
        return (name, None)
    return (name, before + after)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--rom-dir", default=os.environ.get("FN64_ROM_CORPUS_DIR"))
    parser.add_argument("--jobs", type=int, default=6)
    arguments = parser.parse_args()
    if not arguments.rom_dir:
        parser.error("--rom-dir required (or set FN64_ROM_CORPUS_DIR)")
    if not os.access(PROBE, os.X_OK):
        parser.error(
            "build first: cargo build --release -p fn64-discover "
            "--example probe_overlay_min_records"
        )
    roms = sorted(glob.glob(os.path.join(arguments.rom_dir, "*.z64")))
    if not roms:
        parser.error(f"no .z64 inputs in {arguments.rom_dir}")

    print("rom\ttables_before\tintervals_before\ttables_after\tintervals_after")
    failed = 0
    with concurrent.futures.ThreadPoolExecutor(max_workers=arguments.jobs) as pool:
        for name, values in pool.map(measure, roms):
            if values is None:
                failed += 1
                print(f"{name}\tERR\tERR\tERR\tERR")
                continue
            print(f"{name}\t{values[0]}\t{values[1]}\t{values[2]}\t{values[3]}")
    print(
        f"measured {len(roms)} ROMs, {failed} unreadable", file=sys.stderr
    )


main()
