#!/usr/bin/env python3
"""Measure real RT64 port coverage, as distinct from digest-verified credit.

`docs/rt64-port-inventory.json` marks a file `ported` when its upstream
SHA-256 matches the pinned commit. That proves the upstream file has not
drifted; it proves nothing about reproduced behavior, and it credits a
whole file for a partial port.

This tool reports what the Rust modules actually claim to have ported, by
parsing their upstream line-range citations.

Coverage here is an UPPER bound on ported behavior (a citation is a claim,
not a proof) and a LOWER bound on effort (logic may be ported uncited).
It is not a parity measurement. See docs/RT64-PORT-HONEST-INVENTORY.md.

Usage:
    python3 tools/rt64_port_coverage.py            # report
    python3 tools/rt64_port_coverage.py --json     # machine-readable
    python3 tools/rt64_port_coverage.py --check    # fail if coverage regressed
"""
from __future__ import annotations

import argparse
import json
import os
import re
import sys
from collections import defaultdict
from pathlib import Path

INVENTORY = Path("docs/rt64-port-inventory.json")
BASELINE = Path("docs/rt64-port-coverage-baseline.json")

SRC_EXT = r"(?:cpp|h|hlsl|hlsli|mm|c)"
RX_RANGE = re.compile(rf"((?:src|include)/[A-Za-z0-9_/.\-]+\.{SRC_EXT}):(\d+)\s*-\s*(\d+)")
RX_SINGLE = re.compile(rf"((?:src|include)/[A-Za-z0-9_/.\-]+\.{SRC_EXT}):(\d+)(?!\s*[\d\-])")
RX_WHOLE = re.compile(
    rf"`?((?:src|include)/[A-Za-z0-9_/.\-]+\.{SRC_EXT})`?[^\n]{{0,140}}?whole[- ]file"
)
RX_BASE_RANGE = re.compile(rf"\b([A-Za-z0-9_\-]+\.{SRC_EXT}):(\d+)\s*-\s*(\d+)")
RX_BASE_SINGLE = re.compile(rf"\b([A-Za-z0-9_\-]+\.{SRC_EXT}):(\d+)(?!\s*[\d\-])")

MAX_SPAN = 20000  # reject absurd ranges from malformed citations


def measure(root: Path) -> dict:
    inventory = json.loads((root / INVENTORY).read_text())
    lines = {f["path"]: f["sources"]["port"]["lines"] for f in inventory["files"]}
    states = {f["path"]: f["port_state"] for f in inventory["files"]}

    # Resolve bare basenames only when unambiguous across the corpus.
    by_base: dict[str, list[str]] = defaultdict(list)
    for path in lines:
        by_base[os.path.basename(path)].append(path)

    cited: dict[str, set[int]] = defaultdict(set)
    whole: set[str] = set()

    def add(path: str, start: int, end: int) -> None:
        if path in lines and end >= start and end - start < MAX_SPAN:
            cited[path].update(range(start, end + 1))

    def resolve(base: str) -> str | None:
        candidates = by_base.get(base, [])
        return candidates[0] if len(candidates) == 1 else None

    for rs in sorted((root / "crates").rglob("*.rs")):
        try:
            text = rs.read_text(errors="replace")
        except OSError:
            continue
        for m in RX_RANGE.finditer(text):
            add(m.group(1), int(m.group(2)), int(m.group(3)))
        for m in RX_SINGLE.finditer(text):
            add(m.group(1), int(m.group(2)), int(m.group(2)))
        for m in RX_WHOLE.finditer(text):
            whole.add(m.group(1))
        for m in RX_BASE_RANGE.finditer(text):
            if (p := resolve(m.group(1))):
                add(p, int(m.group(2)), int(m.group(3)))
        for m in RX_BASE_SINGLE.finditer(text):
            if (p := resolve(m.group(1))):
                add(p, int(m.group(2)), int(m.group(2)))

    files = {}
    for path, state in states.items():
        if state != "ported":
            continue
        total = lines.get(path, 0)
        covered = total if path in whole else min(
            len([n for n in cited.get(path, ()) if 1 <= n <= total]), total
        )
        files[path] = {
            "lines": total,
            "cited": covered,
            "ratio": round(covered / total, 4) if total else 0.0,
        }

    corpus = sum(lines.values())
    ported = sum(v["lines"] for v in files.values())
    covered = sum(v["cited"] for v in files.values())
    return {
        "corpus_lines": corpus,
        "ported_files": len(files),
        "ported_lines": ported,
        "cited_lines": covered,
        "digest_credit_pct": round(100 * ported / corpus, 1) if corpus else 0.0,
        "honest_coverage_pct": round(100 * covered / corpus, 1) if corpus else 0.0,
        "files": files,
    }


def report(result: dict) -> None:
    print(f"corpus                {result['corpus_lines']:6d} lines")
    print(
        f"digest-credited       {result['ported_lines']:6d} lines "
        f"({result['digest_credit_pct']}%)  <- inventory headline"
    )
    print(
        f"line-cited coverage   {result['cited_lines']:6d} lines "
        f"({result['honest_coverage_pct']}%)  <- honest"
    )
    print()

    order = [
        ("0% (named, no range)", lambda r: r == 0),
        ("1-25%", lambda r: 0 < r <= 0.25),
        ("26-50%", lambda r: 0.25 < r <= 0.50),
        ("51-75%", lambda r: 0.50 < r <= 0.75),
        ("76-99%", lambda r: 0.75 < r < 0.999),
        ("100%", lambda r: r >= 0.999),
    ]
    print(f"{'coverage bucket':24s} {'files':>6s} {'lines':>8s}")
    for label, pred in order:
        sel = [v for v in result["files"].values() if pred(v["ratio"])]
        print(f"  {label:22s} {len(sel):6d} {sum(v['lines'] for v in sel):8d}")

    print()
    print("largest over-credited (<=25% cited, >=150 lines):")
    worst = sorted(
        (
            (p, v)
            for p, v in result["files"].items()
            if v["ratio"] <= 0.25 and v["lines"] >= 150
        ),
        key=lambda kv: -kv[1]["lines"],
    )
    for path, v in worst[:15]:
        print(f"  {v['lines']:6d} lines  {100*v['ratio']:5.1f}%   {path}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--json", action="store_true", help="emit machine-readable JSON")
    parser.add_argument(
        "--check", action="store_true", help="fail if coverage fell below the baseline"
    )
    parser.add_argument(
        "--update-baseline", action="store_true", help="rewrite the baseline file"
    )
    args = parser.parse_args()

    root = Path(__file__).resolve().parent.parent
    result = measure(root)

    if args.json:
        json.dump(result, sys.stdout, indent=1, sort_keys=True)
        print()
        return 0

    if args.update_baseline:
        (root / BASELINE).write_text(
            json.dumps(
                {
                    "cited_lines": result["cited_lines"],
                    "honest_coverage_pct": result["honest_coverage_pct"],
                },
                indent=1,
            )
            + "\n"
        )
        print(f"baseline updated: {result['cited_lines']} cited lines")
        return 0

    report(result)

    if args.check:
        path = root / BASELINE
        if not path.exists():
            print(f"\nno baseline at {BASELINE}; run --update-baseline", file=sys.stderr)
            return 1
        baseline = json.loads(path.read_text())
        if result["cited_lines"] < baseline["cited_lines"]:
            print(
                f"\nFAIL: cited coverage fell {baseline['cited_lines']} -> "
                f"{result['cited_lines']}",
                file=sys.stderr,
            )
            return 1
        print(f"\nOK: cited coverage {result['cited_lines']} >= baseline "
              f"{baseline['cited_lines']}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
