#!/usr/bin/env python3
"""Summarize path-free exclusive costs from an xctrace time-profile export."""

from __future__ import annotations

import argparse
import json
import pathlib
import sys
import xml.etree.ElementTree as ET
from collections import Counter


def _resolve(element: ET.Element, ids: dict[str, ET.Element]) -> ET.Element:
    reference = element.get("ref")
    if reference is None:
        return element
    try:
        return ids[reference]
    except KeyError as error:
        raise ValueError(f"xctrace XML references missing id {reference!r}") from error


def _child(
    element: ET.Element, tag: str, ids: dict[str, ET.Element]
) -> ET.Element | None:
    element = _resolve(element, ids)
    child = element.find(tag)
    return None if child is None else _resolve(child, ids)


def _process_name(row: ET.Element, ids: dict[str, ET.Element]) -> str:
    process = _child(row, "process", ids)
    if process is None:
        raise ValueError("time-profile row has no process")
    return process.get("fmt", "").split(" (", 1)[0]


def _frame_name(frame: ET.Element, ids: dict[str, ET.Element]) -> str:
    frame = _resolve(frame, ids)
    return frame.get("name") or frame.get("fmt") or "<unknown>"


def _frame_binary(frame: ET.Element, ids: dict[str, ET.Element]) -> str | None:
    binary = _child(_resolve(frame, ids), "binary", ids)
    return None if binary is None else binary.get("name")


def _frames(row: ET.Element, ids: dict[str, ET.Element]) -> list[ET.Element]:
    tagged = _child(row, "tagged-backtrace", ids)
    if tagged is None:
        return []
    backtrace = _child(tagged, "backtrace", ids)
    if backtrace is None:
        return []
    return [_resolve(frame, ids) for frame in backtrace.findall("frame")]


def _ranked(counter: Counter[str], limit: int) -> list[dict[str, object]]:
    return [
        {"symbol": symbol, "weight_ms": weight_ns / 1_000_000.0}
        for symbol, weight_ns in counter.most_common(limit)
    ]


def summarize(
    xml_text: str,
    image: str = "fn64",
    process: str | None = None,
    leaf_patterns: tuple[str, ...] = (),
    limit: int = 30,
) -> dict[str, object]:
    root = ET.fromstring(xml_text)
    ids = {
        identifier: element
        for element in root.iter()
        if (identifier := element.get("id")) is not None
    }
    exclusive: Counter[str] = Counter()
    caller_costs = {pattern: Counter() for pattern in leaf_patterns}
    leaf_costs: Counter[str] = Counter()
    samples = 0
    weight_ns = 0

    for row in root.iter("row"):
        if process is not None and _process_name(row, ids) != process:
            continue
        weight = _child(row, "weight", ids)
        if weight is None or weight.text is None:
            raise ValueError("time-profile row has no numeric weight")
        row_weight = int(weight.text)
        frames = _frames(row, ids)
        if not frames:
            continue

        samples += 1
        weight_ns += row_weight
        leaf_name = _frame_name(frames[0], ids)
        exclusive[leaf_name] += row_weight

        for pattern in leaf_patterns:
            if pattern not in leaf_name:
                continue
            leaf_costs[pattern] += row_weight
            caller = next(
                (
                    _frame_name(frame, ids)
                    for frame in frames[1:]
                    if _frame_binary(frame, ids) == image
                ),
                "<no-main-image-caller>",
            )
            caller_costs[pattern][caller] += row_weight

    return {
        "schema": "fn64.xctrace-time-profile.v1",
        "image": image,
        "process": process,
        "samples": samples,
        "weight_ms": weight_ns / 1_000_000.0,
        "exclusive": _ranked(exclusive, limit),
        "leaf_callers": {
            pattern: {
                "weight_ms": leaf_costs[pattern] / 1_000_000.0,
                "callers": _ranked(caller_costs[pattern], limit),
            }
            for pattern in leaf_patterns
        },
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("xml", type=pathlib.Path)
    parser.add_argument("--image", default="fn64")
    parser.add_argument("--process")
    parser.add_argument("--leaf", action="append", default=[])
    parser.add_argument("--top", type=int, default=30)
    parser.add_argument("--output", type=pathlib.Path)
    args = parser.parse_args()
    if args.top <= 0:
        parser.error("--top must be positive")
    try:
        result = summarize(
            args.xml.read_text(encoding="utf-8"),
            image=args.image,
            process=args.process,
            leaf_patterns=tuple(args.leaf),
            limit=args.top,
        )
    except (ET.ParseError, OSError, ValueError) as error:
        parser.error(str(error))

    encoded = json.dumps(result, indent=2, sort_keys=True) + "\n"
    if args.output is None:
        sys.stdout.write(encoded)
    else:
        args.output.write_text(encoded, encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
