#!/usr/bin/env python3
"""Compare two loader treatments without promoting either to discovery authority."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path
import re
import struct
import sys
from typing import NoReturn


SCHEMA = "fn64.ghidra-bank-function-inventory"
REPORT_SCHEMA = "fn64.ghidra-loader-ab"
MAX_INPUT_BYTES = 16 * 1024 * 1024
SHA256 = re.compile(r"[0-9a-f]{64}\Z")
BANK_TOKEN = re.compile(r"[A-Za-z0-9._+\-]{1,128}\Z")
TOP_FIELDS = {
    "schema",
    "schema_version",
    "candidate_only",
    "provenance",
    "input",
    "memory_blocks",
    "entry_point_count",
    "entry_points_sha256",
    "entry_points",
    "rejected_function_count",
    "rejected_functions_sha256",
    "rejected_functions",
    "function_count",
    "function_inventory_sha256",
    "functions",
}
INPUT_FIELDS = {
    "normalized_rom_sha256",
    "bank",
    "bank_bytes_sha256",
    "context_bytes_sha256",
    "mapping_sha256",
    "va_start",
    "va_end",
    "context_start",
    "context_end",
}
BLOCK_FIELDS = {
    "va_start",
    "va_end",
    "overlap_start",
    "overlap_end",
    "read",
    "write",
    "execute",
    "initialized",
}


def fail(message: str) -> NoReturn:
    raise SystemExit(f"ghidra loader A/B comparator: {message}")


def canonical_bytes(value: object) -> bytes:
    return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()


def exact_fields(value: dict[str, object], expected: set[str], label: str) -> None:
    if set(value) != expected:
        fail(
            f"{label} fields differ: missing={sorted(expected - set(value))} "
            f"unknown={sorted(set(value) - expected)}"
        )


def read_json(path_value: str, label: str) -> dict[str, object]:
    path = Path(path_value)
    if not path.is_absolute() or path.is_symlink() or not path.is_file():
        fail(f"{label} must be an absolute regular non-symlink file")
    size = path.stat().st_size
    if size <= 0 or size > MAX_INPUT_BYTES:
        fail(f"{label} size is outside 1..={MAX_INPUT_BYTES}")
    data = path.read_bytes()
    if len(data) != size:
        fail(f"{label} changed while reading")

    def pairs(items: list[tuple[str, object]]) -> dict[str, object]:
        result: dict[str, object] = {}
        for key, value in items:
            if key in result:
                fail(f"{label} contains duplicate field {key}")
            result[key] = value
        return result

    try:
        value = json.loads(data, object_pairs_hook=pairs)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        fail(f"{label} is not strict UTF-8 JSON: {error}")
    if not isinstance(value, dict):
        fail(f"{label} must contain one JSON object")
    return value


def u32(value: object, label: str) -> int:
    if type(value) is not int or value < 0 or value > 0xFFFF_FFFF:
        fail(f"{label} must be a u32")
    return value


def exclusive_u32(value: object, label: str) -> int:
    if type(value) is not int or value < 1 or value > 0x1_0000_0000:
        fail(f"{label} must be an exclusive u32 end")
    return value


def digest_functions(functions: list[tuple[int, tuple[tuple[int, int], ...]]]) -> str:
    digest = hashlib.sha256(b"fn64.ghidra-bank-function-inventory.v1\0")
    digest.update(struct.pack("<Q", len(functions)))
    for entry, ranges in functions:
        digest.update(struct.pack("<I", entry))
        digest.update(struct.pack("<Q", len(ranges)))
        for start, end in ranges:
            digest.update(struct.pack("<II", start, end))
    return digest.hexdigest()


def digest_entry_points(entry_points: list[int]) -> str:
    digest = hashlib.sha256(b"fn64.ghidra-bank-entry-points.v1\0")
    digest.update(struct.pack("<Q", len(entry_points)))
    for entry_point in entry_points:
        digest.update(struct.pack("<I", entry_point))
    return digest.hexdigest()


def digest_rejected_functions(rejected: list[tuple[int, str]]) -> str:
    digest = hashlib.sha256(b"fn64.ghidra-bank-rejected-functions.v1\0")
    digest.update(struct.pack("<Q", len(rejected)))
    for entry, reason in rejected:
        encoded = reason.encode()
        digest.update(struct.pack("<I", entry))
        digest.update(struct.pack("<Q", len(encoded)))
        digest.update(encoded)
    return digest.hexdigest()


def validate_inventory(
    value: dict[str, object], expected_lane: str, expected_phase: str, label: str
) -> dict[str, object]:
    exact_fields(value, TOP_FIELDS, label)
    if value["schema"] != SCHEMA or type(value["schema_version"]) is not int or value["schema_version"] != 4:
        fail(f"{label} has an unsupported schema")
    if value["candidate_only"] is not True:
        fail(f"{label} is not candidate-only")

    provenance = value["provenance"]
    if not isinstance(provenance, dict):
        fail(f"{label} provenance must be an object")
    exact_fields(provenance, {"lane", "phase", "source_sha256"}, f"{label} provenance")
    if provenance["lane"] != expected_lane or provenance["phase"] != expected_phase:
        fail(f"{label} has the wrong lane or phase")
    if not isinstance(provenance["source_sha256"], str) or not SHA256.fullmatch(
        provenance["source_sha256"]
    ):
        fail(f"{label} has an invalid provenance digest")

    input_value = value["input"]
    if not isinstance(input_value, dict):
        fail(f"{label} input must be an object")
    exact_fields(input_value, INPUT_FIELDS, f"{label} input")
    for field in (
        "normalized_rom_sha256",
        "bank_bytes_sha256",
        "context_bytes_sha256",
        "mapping_sha256",
    ):
        if not isinstance(input_value[field], str) or not SHA256.fullmatch(input_value[field]):
            fail(f"{label} input {field} is not a SHA-256 digest")
    bank = input_value["bank"]
    if not isinstance(bank, str) or BANK_TOKEN.fullmatch(bank) is None or bank in {".", ".."}:
        fail(f"{label} has an invalid bank label")
    context_start = u32(input_value["context_start"], f"{label} context_start")
    context_end = u32(input_value["context_end"], f"{label} context_end")
    va_start = u32(input_value["va_start"], f"{label} va_start")
    va_end = u32(input_value["va_end"], f"{label} va_end")
    if (
        context_start >= context_end
        or va_start < context_start
        or va_start >= va_end
        or va_end > context_end
        or any(value_ & 3 for value_ in (context_start, context_end, va_start, va_end))
    ):
        fail(f"{label} has invalid context/bank geometry")

    blocks_value = value["memory_blocks"]
    if not isinstance(blocks_value, list) or not blocks_value:
        fail(f"{label} has no memory blocks")
    blocks: list[dict[str, object]] = []
    expected_overlap = context_start
    previous_key: tuple[int, int, int, int] | None = None
    for index, block in enumerate(blocks_value):
        if not isinstance(block, dict):
            fail(f"{label} memory block {index} must be an object")
        exact_fields(block, BLOCK_FIELDS, f"{label} memory block {index}")
        start = u32(block["va_start"], f"{label} block start")
        end = exclusive_u32(block["va_end"], f"{label} block end")
        overlap_start = u32(block["overlap_start"], f"{label} overlap start")
        overlap_end = u32(block["overlap_end"], f"{label} overlap end")
        if start >= end or overlap_start != max(start, context_start) or overlap_end != min(end, context_end):
            fail(f"{label} memory block {index} has invalid clipped geometry")
        if overlap_start != expected_overlap or overlap_start >= overlap_end:
            fail(f"{label} memory blocks do not exactly cover the context")
        expected_overlap = overlap_end
        for field in ("read", "write", "execute", "initialized"):
            if type(block[field]) is not bool:
                fail(f"{label} memory block {index} {field} must be boolean")
        if block["read"] is not True:
            fail(f"{label} context contains a non-readable memory block")
        key = (start, end, overlap_start, overlap_end)
        if previous_key is not None and key <= previous_key:
            fail(f"{label} memory blocks are not canonical")
        previous_key = key
        blocks.append(block)
    if expected_overlap != context_end:
        fail(f"{label} memory blocks do not reach context_end")

    entry_points_value = value["entry_points"]
    if not isinstance(entry_points_value, list):
        fail(f"{label} entry_points must be an array")
    entry_points: list[int] = []
    previous_entry_point = -1
    for index, item in enumerate(entry_points_value):
        entry_point = u32(item, f"{label} entry point {index}")
        if (
            entry_point <= previous_entry_point
            or entry_point < va_start
            or entry_point >= va_end
            or entry_point & 3
        ):
            fail(f"{label} entry points are not canonical in-bank words")
        previous_entry_point = entry_point
        entry_points.append(entry_point)
    if type(value["entry_point_count"]) is not int or value["entry_point_count"] != len(entry_points):
        fail(f"{label} entry_point_count is inconsistent")
    entry_points_sha = digest_entry_points(entry_points)
    if value["entry_points_sha256"] != entry_points_sha:
        fail(f"{label} entry point digest is inconsistent")

    rejected_value = value["rejected_functions"]
    if not isinstance(rejected_value, list):
        fail(f"{label} rejected_functions must be an array")
    rejected: list[tuple[int, str]] = []
    previous_rejected = -1
    for index, item in enumerate(rejected_value):
        if not isinstance(item, dict):
            fail(f"{label} rejected function {index} must be an object")
        exact_fields(item, {"entry", "reason"}, f"{label} rejected function {index}")
        entry = u32(item["entry"], f"{label} rejected function entry")
        if entry <= previous_rejected or entry < va_start or entry >= va_end or entry & 3:
            fail(f"{label} rejected function entries are not canonical in-bank words")
        if item["reason"] != "non_word_body_range":
            fail(f"{label} rejected function has an unsupported reason")
        previous_rejected = entry
        rejected.append((entry, item["reason"]))
    if type(value["rejected_function_count"]) is not int or value["rejected_function_count"] != len(rejected):
        fail(f"{label} rejected_function_count is inconsistent")
    rejected_sha = digest_rejected_functions(rejected)
    if value["rejected_functions_sha256"] != rejected_sha:
        fail(f"{label} rejected function digest is inconsistent")

    functions_value = value["functions"]
    if not isinstance(functions_value, list):
        fail(f"{label} functions must be an array")
    functions: list[tuple[int, tuple[tuple[int, int], ...]]] = []
    previous_entry = -1
    for function_index, function in enumerate(functions_value):
        if not isinstance(function, dict):
            fail(f"{label} function {function_index} must be an object")
        exact_fields(function, {"entry", "body_ranges"}, f"{label} function {function_index}")
        entry = u32(function["entry"], f"{label} function entry")
        if entry <= previous_entry or entry < va_start or entry >= va_end or entry & 3:
            fail(f"{label} function entries are not canonical in-bank words")
        previous_entry = entry
        ranges_value = function["body_ranges"]
        if not isinstance(ranges_value, list) or not ranges_value:
            fail(f"{label} function {function_index} has no body ranges")
        ranges: list[tuple[int, int]] = []
        previous_end = -1
        contains_entry = False
        for range_index, range_value in enumerate(ranges_value):
            if not isinstance(range_value, dict):
                fail(f"{label} body range {range_index} must be an object")
            exact_fields(range_value, {"va_start", "va_end"}, f"{label} body range")
            start = u32(range_value["va_start"], f"{label} body start")
            end = u32(range_value["va_end"], f"{label} body end")
            if start < va_start or start >= end or end > va_end or start & 3 or end & 3:
                fail(f"{label} has an invalid body range")
            if start < previous_end:
                fail(f"{label} function body ranges overlap or are unsorted")
            previous_end = end
            contains_entry |= start <= entry < end
            ranges.append((start, end))
        if not contains_entry:
            fail(f"{label} function body does not contain its entry")
        functions.append((entry, tuple(ranges)))
    if type(value["function_count"]) is not int or value["function_count"] != len(functions):
        fail(f"{label} function_count is inconsistent")
    actual_digest = digest_functions(functions)
    if value["function_inventory_sha256"] != actual_digest:
        fail(f"{label} function inventory digest is inconsistent")
    if set(entry for entry, _ in rejected) & set(entry for entry, _ in functions):
        fail(f"{label} accepts and rejects the same function entry")
    return {
        "input": input_value,
        "provenance": provenance,
        "memory_blocks": blocks,
        "entry_points": entry_points,
        "entry_points_sha256": entry_points_sha,
        "rejected_functions": rejected,
        "rejected_functions_sha256": rejected_sha,
        "functions": functions,
        "inventory_sha256": actual_digest,
    }


def merged_ranges(functions: list[tuple[int, tuple[tuple[int, int], ...]]]) -> list[tuple[int, int]]:
    ranges = sorted(range_ for _, body in functions for range_ in body)
    result: list[tuple[int, int]] = []
    for start, end in ranges:
        if result and start <= result[-1][1]:
            result[-1] = (result[-1][0], max(result[-1][1], end))
        else:
            result.append((start, end))
    return result


def word_count(ranges: list[tuple[int, int]]) -> int:
    return sum((end - start) // 4 for start, end in ranges)


def intersection_words(first: list[tuple[int, int]], second: list[tuple[int, int]]) -> int:
    first_index = second_index = total = 0
    while first_index < len(first) and second_index < len(second):
        start = max(first[first_index][0], second[second_index][0])
        end = min(first[first_index][1], second[second_index][1])
        if start < end:
            total += (end - start) // 4
        if first[first_index][1] <= second[second_index][1]:
            first_index += 1
        else:
            second_index += 1
    return total


def phase_metrics(
    binary_functions: dict[int, tuple[tuple[int, int], ...]],
    n64_functions: dict[int, tuple[tuple[int, int], ...]],
    binary_entry_points: set[int],
    n64_entry_points: set[int],
    binary_rejected: set[int],
    n64_rejected: set[int],
) -> dict[str, object]:
    binary_entries = set(binary_functions)
    n64_entries = set(n64_functions)
    common_entries = binary_entries & n64_entries
    exact_body_entries = {
        entry for entry in common_entries if binary_functions[entry] == n64_functions[entry]
    }
    binary_ranges = merged_ranges(list(binary_functions.items()))
    n64_ranges = merged_ranges(list(n64_functions.items()))
    binary_words = word_count(binary_ranges)
    n64_words = word_count(n64_ranges)
    shared_words = intersection_words(binary_ranges, n64_ranges)
    return {
        "entry_points": {
            "binary": sorted(binary_entry_points),
            "n64loaderwv": sorted(n64_entry_points),
            "common": sorted(binary_entry_points & n64_entry_points),
            "binary_only": sorted(binary_entry_points - n64_entry_points),
            "n64loaderwv_only": sorted(n64_entry_points - binary_entry_points),
        },
        "rejected_functions": {
            "binary": sorted(binary_rejected),
            "n64loaderwv": sorted(n64_rejected),
            "common": sorted(binary_rejected & n64_rejected),
            "binary_only": sorted(binary_rejected - n64_rejected),
            "n64loaderwv_only": sorted(n64_rejected - binary_rejected),
        },
        "binary_entry_count": len(binary_entries),
        "n64loaderwv_entry_count": len(n64_entries),
        "common_entries": sorted(common_entries),
        "binary_only_entries": sorted(binary_entries - n64_entries),
        "n64loaderwv_only_entries": sorted(n64_entries - binary_entries),
        "exact_body_entries": sorted(exact_body_entries),
        "differing_body_entries": sorted(common_entries - exact_body_entries),
        "body_words": {
            "binary": binary_words,
            "n64loaderwv": n64_words,
            "intersection": shared_words,
            "union": binary_words + n64_words - shared_words,
            "binary_only": binary_words - shared_words,
            "n64loaderwv_only": n64_words - shared_words,
        },
    }


def main(arguments: list[str]) -> None:
    if len(arguments) != 6:
        fail("usage: compare-snapshot-loader-ab.py BINARY_PRE BINARY_POST N64_PRE N64_POST OUT")
    labels = ("binary pre", "binary post", "N64 pre", "N64 post")
    expected = (
        ("binary-loader", "pre"),
        ("binary-loader", "post"),
        ("n64loaderwv", "pre"),
        ("n64loaderwv", "post"),
    )
    inventories = [
        validate_inventory(read_json(path, label), lane, phase, label)
        for path, label, (lane, phase) in zip(arguments[1:5], labels, expected)
    ]
    common_input = inventories[0]["input"]
    if any(inventory["input"] != common_input for inventory in inventories[1:]):
        fail("the four inventories do not bind the same input context")
    for lane_start in (0, 2):
        if inventories[lane_start]["provenance"]["source_sha256"] != inventories[lane_start + 1]["provenance"]["source_sha256"]:
            fail("a lane changed provenance between pre and post analysis")
        if inventories[lane_start]["memory_blocks"] != inventories[lane_start + 1]["memory_blocks"]:
            fail("a lane changed its memory map during analysis")

    binary_pre, binary_post, n64_pre, n64_post = inventories
    binary_pre_functions = dict(binary_pre["functions"])
    n64_pre_functions = dict(n64_pre["functions"])
    binary_functions = dict(binary_post["functions"])
    n64_functions = dict(n64_post["functions"])

    metrics = {
        "pre_analysis": phase_metrics(
            binary_pre_functions,
            n64_pre_functions,
            set(binary_pre["entry_points"]),
            set(n64_pre["entry_points"]),
            {entry for entry, _ in binary_pre["rejected_functions"]},
            {entry for entry, _ in n64_pre["rejected_functions"]},
        ),
        "post_analysis": phase_metrics(
            binary_functions,
            n64_functions,
            set(binary_post["entry_points"]),
            set(n64_post["entry_points"]),
            {entry for entry, _ in binary_post["rejected_functions"]},
            {entry for entry, _ in n64_post["rejected_functions"]},
        ),
        "memory_map_equal": binary_post["memory_blocks"] == n64_post["memory_blocks"],
    }
    inventory_sha256 = {
        "binary_pre": binary_pre["inventory_sha256"],
        "binary_post": binary_post["inventory_sha256"],
        "n64loaderwv_pre": n64_pre["inventory_sha256"],
        "n64loaderwv_post": n64_post["inventory_sha256"],
        "binary_pre_entry_points": binary_pre["entry_points_sha256"],
        "binary_post_entry_points": binary_post["entry_points_sha256"],
        "n64loaderwv_pre_entry_points": n64_pre["entry_points_sha256"],
        "n64loaderwv_post_entry_points": n64_post["entry_points_sha256"],
        "binary_pre_rejected_functions": binary_pre["rejected_functions_sha256"],
        "binary_post_rejected_functions": binary_post["rejected_functions_sha256"],
        "n64loaderwv_pre_rejected_functions": n64_pre["rejected_functions_sha256"],
        "n64loaderwv_post_rejected_functions": n64_post["rejected_functions_sha256"],
    }
    memory_map_sha256 = {
        "binary": hashlib.sha256(canonical_bytes(binary_post["memory_blocks"])).hexdigest(),
        "n64loaderwv": hashlib.sha256(canonical_bytes(n64_post["memory_blocks"])).hexdigest(),
    }
    semantic_wire = {
        "schema": REPORT_SCHEMA,
        "schema_version": 1,
        "input": common_input,
        "inventory_sha256": inventory_sha256,
        "memory_map_sha256": memory_map_sha256,
        "metrics": metrics,
    }
    report = {
        "schema": REPORT_SCHEMA,
        "schema_version": 1,
        "role": "differential_comparison",
        "authority": "candidate_only",
        "context": "shared_mapped_bytes",
        "input": common_input,
        "lane_provenance": {
            "binary_loader_sha256": binary_post["provenance"]["source_sha256"],
            "n64loaderwv_sha256": n64_post["provenance"]["source_sha256"],
        },
        "inventory_sha256": inventory_sha256,
        "memory_map_sha256": memory_map_sha256,
        "metrics": metrics,
        "semantic_sha256": hashlib.sha256(canonical_bytes(semantic_wire)).hexdigest(),
    }
    output = Path(arguments[5])
    if not output.is_absolute() or output.is_symlink() or output.exists() or not output.parent.is_dir():
        fail("OUT must be an absent absolute path under an existing directory")
    output.write_bytes(canonical_bytes(report))


if __name__ == "__main__":
    main(sys.argv)
