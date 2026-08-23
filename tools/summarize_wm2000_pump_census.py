#!/usr/bin/env python3
"""Summarize WM2000 swap-to-swap latency from a pump-census sequence dump."""

from __future__ import annotations

import argparse
import json
import math
import pathlib
import re
import sys
from collections import Counter
from dataclasses import dataclass


SEQUENCE_PREFIX = "[pump-seq] "
RENDERER_RE = re.compile(r"^\[pump-census\] RENDERER: (\S+)$", re.MULTILINE)
BUDGET_MS = 1000.0 / 30.0


@dataclass(frozen=True)
class Pump:
    index: int
    wall_ms: float
    swapped: bool


def percentile(values: list[float], fraction: float) -> float:
    if not values:
        raise ValueError("a percentile requires at least one value")
    ordered = sorted(values)
    rank = max(1, math.ceil(fraction * len(ordered)))
    return ordered[rank - 1]


def parse_pumps(text: str) -> list[Pump]:
    pumps: list[Pump] = []
    for line in text.splitlines():
        if not line.startswith(SEQUENCE_PREFIX):
            continue
        fields = line[len(SEQUENCE_PREFIX) :].split(",")
        if len(fields) != 15:
            raise ValueError(f"pump sequence row has {len(fields)} fields, expected 15")
        pump = Pump(index=int(fields[0]), wall_ms=float(fields[1]), swapped=fields[3] == "1")
        if pump.index != len(pumps):
            raise ValueError(
                f"pump sequence is not contiguous: expected index {len(pumps)}, got {pump.index}"
            )
        pumps.append(pump)
    if not pumps:
        raise ValueError(
            "no [pump-seq] rows found; set FN64_PUMP_CENSUS_SEQUENCE equal to "
            "FN64_PUMP_CENSUS_PUMPS"
        )
    return pumps


def summarize(text: str) -> dict[str, object]:
    renderer_match = RENDERER_RE.search(text)
    if renderer_match is None:
        raise ValueError("pump census renderer identity is missing")
    renderer = renderer_match.group(1)
    pumps = parse_pumps(text)
    swap_indices = [pump.index for pump in pumps if pump.swapped]
    if len(swap_indices) < 2:
        raise ValueError("at least two post-warmup VI swaps are required")

    gaps = [current - previous for previous, current in zip(swap_indices, swap_indices[1:])]
    drawn_ms = [
        sum(pump.wall_ms for pump in pumps[previous + 1 : current + 1])
        for previous, current in zip(swap_indices, swap_indices[1:])
    ]
    gap_counts = Counter(gaps)
    over_budget = sum(value > BUDGET_MS for value in drawn_ms)

    return {
        "schema": "fn64.wm2000-swap-latency.v1",
        "renderer": renderer,
        "pumps": len(pumps),
        "swaps": len(swap_indices),
        "drawn_frames": len(drawn_ms),
        "swap_gap_histogram": {str(gap): gap_counts[gap] for gap in sorted(gap_counts)},
        "gap_two_fraction": gap_counts[2] / len(gaps),
        "budget_ms": BUDGET_MS,
        "drawn_frame_ms": {
            "mean": sum(drawn_ms) / len(drawn_ms),
            "p50": percentile(drawn_ms, 0.50),
            "p95": percentile(drawn_ms, 0.95),
            "max": max(drawn_ms),
        },
        "over_budget": {
            "count": over_budget,
            "fraction": over_budget / len(drawn_ms),
        },
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("log", type=pathlib.Path)
    parser.add_argument("--output", type=pathlib.Path)
    args = parser.parse_args()
    try:
        result = summarize(args.log.read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        parser.error(str(error))

    encoded = json.dumps(result, indent=2, sort_keys=True) + "\n"
    if args.output is not None:
        args.output.write_text(encoded, encoding="utf-8")
    else:
        sys.stdout.write(encoded)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
