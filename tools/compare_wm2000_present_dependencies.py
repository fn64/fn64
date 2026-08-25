#!/usr/bin/env python3
"""Require exact per-pump presentation-dependency parity for a WM2000 A/B."""

from __future__ import annotations

import argparse
import json
import pathlib

from summarize_wm2000_pump_census import (
    parse_present_dependencies,
    present_identity_sha256,
)


EXPECTED_PUMPS = 1600


def compare_logs(
    observe_text: str, suppress_text: str, expected_pumps: int = EXPECTED_PUMPS
) -> dict[str, object]:
    observe = parse_present_dependencies(observe_text, expected_pumps)
    suppress = parse_present_dependencies(suppress_text, expected_pumps)
    if not observe or observe[0].mode != "Observe":
        raise ValueError("first log must contain an Observe dependency receipt")
    if not suppress or suppress[0].mode != "Suppress":
        raise ValueError("second log must contain a Suppress dependency receipt")
    for left, right in zip(observe, suppress):
        if left.pump != right.pump:
            raise ValueError(
                f"present dependency pump index differs: {left.pump} != {right.pump}"
            )
        if left.canonical_identity() != right.canonical_identity():
            raise ValueError(
                f"present dependency identity differs at pump {left.pump}: "
                f"{left.canonical_identity()!r} != {right.canonical_identity()!r}"
            )
    observe_digest = present_identity_sha256(observe)
    suppress_digest = present_identity_sha256(suppress)
    if observe_digest != suppress_digest:
        raise ValueError("present dependency canonical identity digest differs")
    return {
        "schema": "fn64.wm2000-present-dependency-comparison.v1",
        "pumps": expected_pumps,
        "canonical_identity_sha256": observe_digest,
        "observe": {
            "exact_hits": sum(sample.exact_hit for sample in observe),
            "suppressed": sum(sample.disposition == "Suppress" for sample in observe),
        },
        "suppress": {
            "exact_hits": sum(sample.exact_hit for sample in suppress),
            "suppressed": sum(sample.disposition == "Suppress" for sample in suppress),
        },
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("observe", type=pathlib.Path)
    parser.add_argument("suppress", type=pathlib.Path)
    parser.add_argument("--output", type=pathlib.Path)
    args = parser.parse_args()
    try:
        result = compare_logs(
            args.observe.read_text(encoding="utf-8"),
            args.suppress.read_text(encoding="utf-8"),
        )
    except (OSError, ValueError) as error:
        parser.error(str(error))
    encoded = json.dumps(result, indent=2, sort_keys=True) + "\n"
    if args.output is None:
        print(encoded, end="")
    else:
        args.output.write_text(encoded, encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
