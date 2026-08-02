#!/usr/bin/env python3
"""Mechanically shortlist a novel proof-target ROM from the measured corpus.

"Novel" means no known public decompilation or recompilation project: the
byte-exact rebuild proof on such a ROM demonstrates the pipeline cold, with
no answer key even available. Everything else is scored from corpus
measurements; the ONLY hand-maintained input is the exclusion list of games
with known community projects, which cannot be derived mechanically.

Inputs (all produced by this repo's corpus tooling):
  roms.jsonl      -- scripts/rom-catalog.py output (identity, hazards)
  frontier.jsonl  -- scripts/rom-frontier.py output (discovery outcomes)
  resweep.json    -- diagnose_open_indirects sweep (banks, open sites)

Output: a ranked shortlist. The final pick is made by RUNNING
gate_rom_rebuild on the shortlist and taking the highest measured
roundtripped-code share -- the gate is the selector of record; this script
only orders the queue.
"""

import argparse
import json
import re
import sys

# Games with a known public decomp or recomp project (complete or active).
# Curated 2026-08-01; err toward exclusion -- a false exclusion costs one
# candidate, a false inclusion voids the "novel" claim.
KNOWN_PROJECT_PATTERNS = [
    r"zelda|ocarina|majora",
    r"mario 64|mario64",
    r"paper mario",
    r"mario kart",
    r"mario party",
    r"smash",
    r"kirby",
    r"banjo",
    r"perfect dark",
    r"goldeneye",
    r"donkey kong",
    r"diddy kong",
    r"star fox|starfox|lylat",
    r"pokemon|pocket monsters",
    r"f-zero",
    r"wave race",
    r"yoshi",
    r"conker",
    r"dinosaur planet",
    r"bomberman 64|baku bomberman",
    r"goemon|mystical ninja",
    r"quest 64|holy magic century",
    r"mischief makers|yuke yuke",
    r"silicon valley",
    r"rocket - robot|rocket robot",
    r"chameleon twist",
    r"wrestlemania 2000|no mercy|virtual pro",  # AKI: WM2000Recomp etc.
    r"turok",  # official remasters built from source-level work
    r"doom|quake|hexen|duke nukem",  # ports with released source
    r"animal (forest|crossing)|doubutsu",
    r"harvest moon",
    r"castlevania",
    r"superman",  # notorious; has an active decomp
    r"blast corps",
    r"jet force",
    r"glover",
    r"gauntlet",
    r"rayman",
    r"snowboard kids",
    r"space station",
    r"aidyn",
    r"body harvest",
    r"shadows of the empire|rogue squadron|battle for naboo|episode i",
    r"tetrisphere",  # decomp in progress
]


def known_project(name):
    lowered = name.lower()
    return any(re.search(pattern, lowered) for pattern in KNOWN_PROJECT_PATTERNS)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--roms", required=True, help="roms.jsonl from rom-catalog.py")
    parser.add_argument("--frontier", required=True, help="frontier.jsonl from rom-frontier.py")
    parser.add_argument("--resweep", required=True, help="resweep.json open-indirect sweep")
    parser.add_argument("--top", type=int, default=10)
    args = parser.parse_args()

    roms = {}
    for line in open(args.roms):
        row = json.loads(line)
        roms[row["normalized_rom_sha256"]] = row
    frontier = {}
    for line in open(args.frontier):
        row = json.loads(line)
        frontier[row["normalized_rom_sha256"]] = row
    resweep = {}
    for row in json.load(open(args.resweep)):
        if "error" in row:
            continue
        # resweep rows key by truncated sha; index by prefix.
        resweep[row["sha"]] = row

    candidates = []
    excluded_known = 0
    excluded_geometry = 0
    excluded_compressed = 0
    for sha, rom in roms.items():
        name = rom.get("dat_name") or rom["internal_name"]
        if known_project(name):
            excluded_known += 1
            continue
        front = frontier.get(sha)
        # `geometry_failure` is a taxonomy, not a boolean. Complete geometry
        # means either the overlay table was recovered, or the ROM is
        # resident-code with no overlay system at all (nothing to find). A
        # loader_stub with no recovered table loads code by a mechanism we
        # did not prove -- geometry incomplete, excluded.
        geometry_complete = front is not None and (
            front.get("geometry_failure") == "recovered"
            or (
                front.get("geometry_failure") == "no_candidate_table_found"
                and front.get("class") == "resident_code"
            )
        )
        if not geometry_complete:
            excluded_geometry += 1
            continue
        # Compression markers void the direct ROM-affine round-trip for the
        # marked payloads; keep the first proof unencumbered. (LZSS/rzip
        # decode exists, but materialized-source classification is v2.)
        if rom.get("compression_markers"):
            excluded_compressed += 1
            continue
        sweep = resweep.get(sha[:12], {})
        banks = sweep.get("banks", front.get("mapped_banks", 0))
        open_sites = sweep.get("open", None)
        score = (
            front.get("proven_target_share", 0.0),
            rom.get("code_run_share", 0.0),
            -banks,  # fewer banks = simpler whole-ROM story
            -(open_sites if open_sites is not None else 10_000),
        )
        candidates.append(
            {
                "name": name,
                "sha": sha[:12],
                "score": score,
                "banks": banks,
                "open_sites": open_sites,
                "proven_target_share": front.get("proven_target_share"),
                "code_run_share": rom.get("code_run_share"),
                "executable_bytes": front.get("executable_bytes"),
                "size_bytes": rom.get("size_bytes"),
                "strategy": front.get("selected_strategy"),
            }
        )

    candidates.sort(key=lambda c: c["score"], reverse=True)
    print(
        f"corpus={len(roms)} excluded: known-project={excluded_known} "
        f"geometry={excluded_geometry} compressed={excluded_compressed} "
        f"candidates={len(candidates)}",
        file=sys.stderr,
    )
    print(
        f"{'rank':>4} {'name':44.44} {'banks':>5} {'open':>5} "
        f"{'proven%':>8} {'code%':>6} {'MiB':>5}"
    )
    for rank, c in enumerate(candidates[: args.top], 1):
        proven = c["proven_target_share"]
        code = c["code_run_share"]
        print(
            f"{rank:>4} {c['name']:44.44} {c['banks']:>5} "
            f"{c['open_sites'] if c['open_sites'] is not None else '?':>5} "
            f"{proven * 100 if proven is not None else -1:>8.2f} "
            f"{code * 100 if code is not None else -1:>6.2f} "
            f"{c['size_bytes'] / 1048576:>5.1f}"
        )
    json.dump(candidates[: args.top], open("novel-shortlist.json", "w"), indent=1)
    print("wrote novel-shortlist.json", file=sys.stderr)


main()
