#!/usr/bin/env python3
"""Summarize path-free exclusive costs from an xctrace profile export."""

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


def _frame_address(frame: ET.Element, ids: dict[str, ET.Element]) -> int | None:
    address = _resolve(frame, ids).get("addr")
    return None if address is None else int(address, 0)


def _frame_image(
    frame: ET.Element, ids: dict[str, ET.Element]
) -> dict[str, object] | None:
    binary = _child(_resolve(frame, ids), "binary", ids)
    if binary is None:
        return None
    result: dict[str, object] = {"name": binary.get("name", "<unknown>")}
    if uuid := binary.get("UUID"):
        result["uuid"] = uuid
    if arch := binary.get("arch"):
        result["arch"] = arch
    if load_address := binary.get("load-addr"):
        result["load_address"] = int(load_address, 0)
    return result


def _frames(row: ET.Element, ids: dict[str, ET.Element]) -> list[ET.Element]:
    tagged = _child(row, "tagged-backtrace", ids)
    if tagged is None:
        return []
    backtrace = _child(tagged, "backtrace", ids)
    if backtrace is None:
        return []
    return [_resolve(frame, ids) for frame in backtrace.findall("frame")]


def _ranked(
    counter: Counter[str], limit: int, value_name: str, divisor: float = 1.0
) -> list[dict[str, object]]:
    return [
        {"symbol": symbol, value_name: weight / divisor}
        for symbol, weight in counter.most_common(limit)
    ]


def _row_weight(
    row: ET.Element, ids: dict[str, ET.Element]
) -> tuple[int, str, str]:
    candidates = [
        ("weight", "nanoseconds", "weight_ms"),
        ("cycle-weight", "cycles", "cycles"),
    ]
    present = [
        (tag, unit, value_name, _child(row, tag, ids))
        for tag, unit, value_name in candidates
        if row.find(tag) is not None
    ]
    if len(present) != 1:
        raise ValueError("profile row must have exactly one numeric weight")
    _, unit, value_name, weight = present[0]
    if weight is None or weight.text is None:
        raise ValueError("profile row has no numeric weight")
    return int(weight.text), unit, value_name


def summarize(
    xml_text: str,
    image: str = "fn64",
    process: str | None = None,
    leaf_patterns: tuple[str, ...] = (),
    limit: int = 30,
    stack_depth: int = 8,
) -> dict[str, object]:
    root = ET.fromstring(xml_text)
    ids = {
        identifier: element
        for element in root.iter()
        if (identifier := element.get("id")) is not None
    }
    exclusive: Counter[str] = Counter()
    caller_costs = {pattern: Counter() for pattern in leaf_patterns}
    call_path_costs = {pattern: Counter() for pattern in leaf_patterns}
    leaf_address_costs = {pattern: Counter() for pattern in leaf_patterns}
    leaf_address_images: dict[tuple[object, ...], dict[str, object]] = {}
    leaf_costs: Counter[str] = Counter()
    samples = 0
    total_weight = 0
    weight_unit: str | None = None
    value_name: str | None = None

    for row in root.iter("row"):
        if process is not None and _process_name(row, ids) != process:
            continue
        row_weight, row_unit, row_value_name = _row_weight(row, ids)
        if weight_unit is not None and weight_unit != row_unit:
            raise ValueError("profile mixes nanosecond and cycle weights")
        weight_unit = row_unit
        value_name = row_value_name
        frames = _frames(row, ids)
        if not frames:
            continue

        samples += 1
        total_weight += row_weight
        leaf_name = _frame_name(frames[0], ids)
        exclusive[leaf_name] += row_weight

        for pattern in leaf_patterns:
            if pattern not in leaf_name:
                continue
            leaf_costs[pattern] += row_weight
            leaf_address = _frame_address(frames[0], ids)
            leaf_image = _frame_image(frames[0], ids)
            if leaf_address is not None and leaf_image is not None:
                image_name = str(leaf_image["name"])
                address_key = (
                    image_name,
                    leaf_image.get("uuid"),
                    leaf_image.get("arch"),
                    leaf_image.get("load_address"),
                    leaf_address,
                )
                leaf_address_costs[pattern][address_key] += row_weight
                leaf_address_images[address_key] = leaf_image
            caller = next(
                (
                    _frame_name(frame, ids)
                    for frame in frames[1:]
                    if _frame_binary(frame, ids) == image
                ),
                "<no-main-image-caller>",
            )
            caller_costs[pattern][caller] += row_weight
            main_image_path = [leaf_name]
            main_image_path.extend(
                _frame_name(frame, ids)
                for frame in frames[1:]
                if _frame_binary(frame, ids) == image
            )
            call_path_costs[pattern][" <- ".join(main_image_path[:stack_depth])] += row_weight

    if weight_unit is None or value_name is None:
        weight_unit = "nanoseconds"
        value_name = "weight_ms"
    divisor = 1_000_000.0 if weight_unit == "nanoseconds" else 1.0

    def ranked_addresses(pattern: str) -> list[dict[str, object]]:
        result = []
        for address_key, weight in leaf_address_costs[pattern].most_common(limit):
            image_name, _, _, _, address = address_key
            image_metadata = leaf_address_images[address_key]
            entry: dict[str, object] = {
                "address": f"0x{address:x}",
                "image": image_name,
                value_name: weight / divisor,
            }
            if uuid := image_metadata.get("uuid"):
                entry["image_uuid"] = uuid
            if arch := image_metadata.get("arch"):
                entry["image_arch"] = arch
            if load_address := image_metadata.get("load_address"):
                load_address = int(load_address)
                if address < load_address:
                    raise ValueError("profile leaf address precedes its image load address")
                entry["image_load_address"] = f"0x{load_address:x}"
                entry["image_offset"] = f"0x{address - load_address:x}"
            result.append(entry)
        return result

    schema = (
        "fn64.xctrace-time-profile.v1"
        if weight_unit == "nanoseconds"
        else "fn64.xctrace-cpu-profile.v1"
    )
    result: dict[str, object] = {
        "schema": schema,
        "image": image,
        "process": process,
        "samples": samples,
        "weight_unit": weight_unit,
        value_name: total_weight / divisor,
        "exclusive": _ranked(exclusive, limit, value_name, divisor),
        "leaf_callers": {
            pattern: {
                value_name: leaf_costs[pattern] / divisor,
                "addresses": ranked_addresses(pattern),
                "callers": _ranked(
                    caller_costs[pattern], limit, value_name, divisor
                ),
                "call_paths": _ranked(
                    call_path_costs[pattern], limit, value_name, divisor
                ),
            }
            for pattern in leaf_patterns
        },
    }
    return result


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("xml", type=pathlib.Path)
    parser.add_argument("--image", default="fn64")
    parser.add_argument("--process")
    parser.add_argument("--leaf", action="append", default=[])
    parser.add_argument("--top", type=int, default=30)
    parser.add_argument("--stack-depth", type=int, default=8)
    parser.add_argument("--output", type=pathlib.Path)
    args = parser.parse_args()
    if args.top <= 0:
        parser.error("--top must be positive")
    if args.stack_depth <= 0:
        parser.error("--stack-depth must be positive")
    try:
        result = summarize(
            args.xml.read_text(encoding="utf-8"),
            image=args.image,
            process=args.process,
            leaf_patterns=tuple(args.leaf),
            limit=args.top,
            stack_depth=args.stack_depth,
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
