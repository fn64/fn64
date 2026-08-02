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

Output: a ranked shortlist. Pass one `--rebuild-report` per shortlisted ROM
after running `gate_rom_rebuild` with `FN64_REBUILD_REPORT`; the final pick is
then the successful report with the greatest absolute number of
roundtripped-code bytes. Absolute coverage is the plan's leverage metric:
it maximizes how much novel code the first proof actually exercises. The
script refuses a partial report set so the selected target cannot be chosen
post hoc.
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


def select_from_rebuild_reports(shortlist, report_paths):
    reports = {}
    for path in report_paths:
        with open(path) as report_file:
            report = json.load(report_file)
        if report.get("schema") != "fn64.rom-rebuild-report.v1":
            raise ValueError(f"{path}: unsupported rebuild-report schema")
        sha = report.get("normalized_rom_sha256", "")
        if len(sha) != 64 or any(char not in "0123456789abcdef" for char in sha):
            raise ValueError(f"{path}: invalid normalized_rom_sha256")
        if sha in reports:
            raise ValueError(f"{path}: duplicate rebuild report for {sha[:12]}")
        classes = (
            report.get("header_ipl3_bytes", -1)
            + report.get("roundtripped_code_bytes", -1)
            + report.get("opaque_bytes", -1)
        )
        if classes != report.get("rom_bytes"):
            raise ValueError(f"{path}: physical byte classes do not cover the ROM")
        if (
            not report.get("digest_match")
            or report.get("differences") != 0
            or report.get("regions_exact") != report.get("regions_attempted")
        ):
            raise ValueError(f"{path}: rebuild gate did not pass exactly")
        reports[sha] = report

    matched = []
    missing = []
    for candidate in shortlist:
        matches = [
            report for sha, report in reports.items() if sha.startswith(candidate["sha"])
        ]
        if len(matches) != 1:
            missing.append(candidate["sha"])
            continue
        matched.append((candidate, matches[0]))
    if missing:
        raise ValueError(
            "rebuild reports must cover the complete shortlist; missing/ambiguous: "
            + ", ".join(missing)
        )
    unused = set(reports) - {
        report["normalized_rom_sha256"] for _, report in matched
    }
    if unused:
        raise ValueError(
            "rebuild reports include ROMs outside the shortlist: "
            + ", ".join(sorted(sha[:12] for sha in unused))
        )

    candidate, report = min(
        matched,
        key=lambda item: (
            -item[1]["roundtripped_code_bytes"],
            item[1]["normalized_rom_sha256"],
        ),
    )
    return {
        "schema": "fn64.novel-rebuild-selection.v1",
        "name": candidate["name"],
        "normalized_rom_sha256": report["normalized_rom_sha256"],
        "roundtripped_code_bytes": report["roundtripped_code_bytes"],
        "rom_bytes": report["rom_bytes"],
        "roundtripped_code_share": (
            report["roundtripped_code_bytes"] / report["rom_bytes"]
        ),
        "shortlist_size": len(shortlist),
        "metric": "max_absolute_roundtripped_code_bytes",
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--roms", required=True, help="roms.jsonl from rom-catalog.py")
    parser.add_argument("--frontier", required=True, help="frontier.jsonl from rom-frontier.py")
    parser.add_argument("--resweep", required=True, help="resweep.json open-indirect sweep")
    parser.add_argument("--top", type=int, default=10)
    parser.add_argument(
        "--rebuild-report",
        action="append",
        default=[],
        help="content-free gate_rom_rebuild JSON receipt; repeat for every shortlisted ROM",
    )
    parser.add_argument(
        "--selection-output",
        help="write the mechanically selected target as content-free JSON",
    )
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
        # `proven_target_share` was dropped as a score term: with
        # proven_entries pinned at 1 for every corpus ROM it equalled
        # 1/distinct_jal_targets, so ranking on it ranked ROMs by having the
        # FEWEST call targets -- the opposite of the intended "most proven".
        # code_run_share leads instead: it measures how much of the image is
        # code the gate can actually exercise.
        score = (
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
        f"{'code%':>6} {'MiB':>5}"
    )
    for rank, c in enumerate(candidates[: args.top], 1):
        code = c["code_run_share"]
        print(
            f"{rank:>4} {c['name']:44.44} {c['banks']:>5} "
            f"{c['open_sites'] if c['open_sites'] is not None else '?':>5} "
            f"{code * 100 if code is not None else -1:>6.2f} "
            f"{c['size_bytes'] / 1048576:>5.1f}"
        )
    shortlist = candidates[: args.top]
    json.dump(shortlist, open("novel-shortlist.json", "w"), indent=1)
    print("wrote novel-shortlist.json", file=sys.stderr)

    if args.rebuild_report:
        if not args.selection_output:
            parser.error("--selection-output is required with --rebuild-report")
        try:
            selection = select_from_rebuild_reports(shortlist, args.rebuild_report)
        except ValueError as error:
            parser.error(str(error))
        with open(args.selection_output, "w") as output:
            json.dump(selection, output, indent=1)
            output.write("\n")
        print(
            "selected "
            f"{selection['name']} sha={selection['normalized_rom_sha256'][:12]} "
            f"roundtripped={selection['roundtripped_code_bytes']}/"
            f"{selection['rom_bytes']} "
            f"({selection['roundtripped_code_share'] * 100:.2f}%)",
            file=sys.stderr,
        )
        print(f"wrote {args.selection_output}", file=sys.stderr)


if __name__ == "__main__":
    main()
