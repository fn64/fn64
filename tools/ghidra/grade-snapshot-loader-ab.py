#!/usr/bin/env python3
"""Grade a boot-bank loader A/B report against a held-out decomp dump."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path
import re
import sys
import tomllib
from typing import NoReturn


REPORT_SCHEMA = "fn64.ghidra-loader-ab-grade"
SHA256 = re.compile(r"[0-9a-f]{64}\Z")
BOOT_ROM_START = 0x1000
BOOT_ROM_END = 0x101000
BOOT_VA_START = 0x80000400
MAX_COMPARISON_BYTES = 16 * 1024 * 1024
MAX_BANK_BYTES = 8 * 1024 * 1024
MAX_ROM_BYTES = 64 * 1024 * 1024
MAX_DUMP_BYTES = 64 * 1024 * 1024


def fail(message: str) -> NoReturn:
    raise SystemExit(f"ghidra loader A/B grade: {message}")


def read_regular(path_value: str, limit: int, label: str) -> tuple[Path, bytes]:
    path = Path(path_value)
    if not path.is_absolute() or path.is_symlink() or not path.is_file():
        fail(f"{label} must be an absolute regular non-symlink file")
    size = path.stat().st_size
    if size <= 0 or size > limit:
        fail(f"{label} size is outside 1..={limit}")
    data = path.read_bytes()
    if len(data) != size:
        fail(f"{label} changed while reading")
    return path, data


def canonical_bytes(value: object) -> bytes:
    return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()


def strict_json(data: bytes, label: str) -> dict[str, object]:
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
        fail(f"{label} is invalid UTF-8 JSON: {error}")
    if not isinstance(value, dict) or canonical_bytes(value) != data:
        fail(f"{label} is not one canonical JSON object")
    return value


def exact_fields(value: dict[str, object], expected: set[str], label: str) -> None:
    if set(value) != expected:
        fail(f"{label} fields differ")


def normalize_rom(data: bytes) -> bytes:
    if len(data) < BOOT_ROM_END or len(data) > MAX_ROM_BYTES or len(data) % 4:
        fail("ROM length cannot contain the complete aligned boot bank")
    magic = data[:4]
    if magic == bytes.fromhex("80371240"):
        return data
    output = bytearray(len(data))
    if magic == bytes.fromhex("40123780"):
        for offset in range(0, len(data), 4):
            output[offset : offset + 4] = data[offset : offset + 4][::-1]
        return bytes(output)
    if magic == bytes.fromhex("37804012"):
        for offset in range(0, len(data), 2):
            output[offset : offset + 2] = data[offset : offset + 2][::-1]
        return bytes(output)
    fail("ROM byte order is not z64, n64, or v64")


def canonical_entries(value: object, label: str) -> list[int]:
    if not isinstance(value, list):
        fail(f"{label} must be an array")
    result: list[int] = []
    prior = -1
    for item in value:
        if type(item) is not int or item < BOOT_VA_START or item > 0xFFFF_FFFF or item & 3:
            fail(f"{label} contains an invalid entry")
        if item <= prior:
            fail(f"{label} is not sorted and unique")
        prior = item
        result.append(item)
    return result


def validated_lane_entries(comparison: dict[str, object]) -> tuple[set[int], set[int]]:
    expected_top = {
        "schema", "schema_version", "role", "authority", "context", "input",
        "lane_provenance", "inventory_sha256", "memory_map_sha256", "metrics",
        "semantic_sha256",
    }
    exact_fields(comparison, expected_top, "comparison")
    if (
        comparison["schema"] != "fn64.ghidra-loader-ab"
        or comparison["schema_version"] != 1
        or comparison["role"] != "differential_comparison"
        or comparison["authority"] != "candidate_only"
        or comparison["context"] != "shared_mapped_bytes"
    ):
        fail("comparison has unsupported authority or schema")
    semantic = {
        "schema": comparison["schema"],
        "schema_version": comparison["schema_version"],
        "input": comparison["input"],
        "inventory_sha256": comparison["inventory_sha256"],
        "memory_map_sha256": comparison["memory_map_sha256"],
        "metrics": comparison["metrics"],
    }
    semantic_sha = hashlib.sha256(canonical_bytes(semantic)).hexdigest()
    if comparison["semantic_sha256"] != semantic_sha:
        fail("comparison semantic digest is inconsistent")
    metrics = comparison["metrics"]
    if not isinstance(metrics, dict) or not isinstance(metrics.get("post_analysis"), dict):
        fail("comparison has no post-analysis metrics")
    post = metrics["post_analysis"]
    common = set(canonical_entries(post.get("common_entries"), "common entries"))
    binary_only = set(canonical_entries(post.get("binary_only_entries"), "Binary-only entries"))
    n64_only = set(canonical_entries(post.get("n64loaderwv_only_entries"), "VW-only entries"))
    if common & binary_only or common & n64_only or binary_only & n64_only:
        fail("comparison entry partitions overlap")
    binary = common | binary_only
    n64 = common | n64_only
    if post.get("binary_entry_count") != len(binary) or post.get("n64loaderwv_entry_count") != len(n64):
        fail("comparison entry counts are inconsistent")
    return binary, n64


def answer_functions(dump_data: bytes) -> tuple[list[dict[str, object]], int]:
    try:
        dump = tomllib.loads(dump_data.decode())
    except (UnicodeDecodeError, tomllib.TOMLDecodeError) as error:
        fail(f"answer-key dump is invalid TOML: {error}")
    sections = dump.get("section")
    if not isinstance(sections, list):
        fail("answer-key dump has no section array")
    functions: list[dict[str, object]] = []
    for section in sections:
        if not isinstance(section, dict):
            fail("answer-key section is not an object")
        rom = section.get("rom")
        vram = section.get("vram")
        if type(rom) is not int or type(vram) is not int:
            fail("answer-key section has invalid ROM/VRAM")
        if not (BOOT_ROM_START <= rom < BOOT_ROM_END):
            continue
        if vram != BOOT_VA_START + rom - BOOT_ROM_START:
            continue
        rows = section.get("functions", [])
        if not isinstance(rows, list):
            fail("answer-key section functions are not an array")
        for row in rows:
            if not isinstance(row, dict):
                fail("answer-key function is not an object")
            name, start, size = row.get("name"), row.get("vram"), row.get("size")
            if not isinstance(name, str) or not name or type(start) is not int or type(size) is not int:
                fail("answer-key function fields are invalid")
            if start & 3 or size < 0 or start < BOOT_VA_START or start + size > BOOT_VA_START + (BOOT_ROM_END - BOOT_ROM_START):
                fail("answer-key function lies outside the aligned boot bank")
            functions.append({"name": name, "start": start, "size": size})
    functions.sort(key=lambda function: function["start"])
    if not functions:
        fail("answer key has no affine boot-bank functions")
    code_end = max(function["start"] + function["size"] for function in functions)
    return functions, code_end


def jal_targets(bank: bytes, code_end: int) -> set[int]:
    result: set[int] = set()
    byte_length = code_end - BOOT_VA_START
    for offset in range(0, byte_length, 4):
        word = int.from_bytes(bank[offset : offset + 4], "big")
        if word >> 26 == 3:
            pc = BOOT_VA_START + offset
            result.add(((pc + 4) & 0xF000_0000) | ((word & 0x03FF_FFFF) << 2))
    return result


def grade(roots: set[int], functions: list[dict[str, object]], code_end: int, jals: set[int]) -> dict[str, object]:
    exact = interior = wrong = open_count = 0
    details: list[dict[str, object]] = []
    for index, function in enumerate(functions):
        start = function["start"]
        end = functions[index + 1]["start"] if index + 1 < len(functions) else code_end
        splits = sorted(root for root in roots if start < root < end)
        if splits:
            root = splits[0]
            classification = "interior_entry" if root in jals else "wrong_split"
            interior += classification == "interior_entry"
            wrong += classification == "wrong_split"
            details.append({"name": function["name"], "start": start, "classification": classification, "root": root})
        elif start in roots:
            exact += 1
            details.append({"name": function["name"], "start": start, "classification": "matched_exact"})
        else:
            open_count += 1
            details.append({"name": function["name"], "start": start, "classification": "open"})
    covered_roots = {
        root
        for index, function in enumerate(functions)
        for root in roots
        if function["start"] <= root < (functions[index + 1]["start"] if index + 1 < len(functions) else code_end)
    }
    return {
        "answer_key_functions": len(functions),
        "candidate_entries_in_code_window": len({root for root in roots if root < code_end}),
        "matched_exact": exact,
        "interior_entries": interior,
        "wrong": wrong,
        "open": open_count,
        "uncovered_entries": sorted(root for root in roots if root < code_end and root not in covered_roots),
        "per_function": details,
    }


def main(arguments: list[str]) -> None:
    if len(arguments) != 6:
        fail("usage: grade-snapshot-loader-ab.py COMPARISON BANK ROM DUMP OUT")
    _, comparison_data = read_regular(arguments[1], MAX_COMPARISON_BYTES, "comparison")
    _, bank = read_regular(arguments[2], MAX_BANK_BYTES, "bank")
    _, rom_data = read_regular(arguments[3], MAX_ROM_BYTES, "ROM")
    _, dump_data = read_regular(arguments[4], MAX_DUMP_BYTES, "answer-key dump")
    comparison = strict_json(comparison_data, "comparison")
    binary_entries, n64_entries = validated_lane_entries(comparison)
    input_value = comparison["input"]
    if not isinstance(input_value, dict):
        fail("comparison input is not an object")
    normalized = normalize_rom(rom_data)
    normalized_sha = hashlib.sha256(normalized).hexdigest()
    if input_value.get("normalized_rom_sha256") != normalized_sha:
        fail("comparison and ROM identities differ")
    if input_value.get("bank") != "boot" or input_value.get("va_start") != BOOT_VA_START or input_value.get("va_end") != BOOT_VA_START + len(bank):
        fail("comparison is not the exact boot-bank geometry")
    expected_bank = normalized[BOOT_ROM_START:BOOT_ROM_END]
    if bank != expected_bank or input_value.get("bank_bytes_sha256") != hashlib.sha256(bank).hexdigest():
        fail("bank bytes do not equal the normalized ROM boot copy")
    functions, code_end = answer_functions(dump_data)
    jals = jal_targets(bank, code_end)
    binary_grade = grade(binary_entries, functions, code_end, jals)
    n64_grade = grade(n64_entries, functions, code_end, jals)
    report = {
        "schema": REPORT_SCHEMA,
        "schema_version": 1,
        "authority": "grading_only",
        "candidate_only": True,
        "input": {
            "normalized_rom_sha256": normalized_sha,
            "bank_sha256": hashlib.sha256(bank).hexdigest(),
            "comparison_sha256": hashlib.sha256(comparison_data).hexdigest(),
            "comparison_semantic_sha256": comparison["semantic_sha256"],
            "answer_key_sha256": hashlib.sha256(dump_data).hexdigest(),
            "code_end": code_end,
        },
        "binary_loader": binary_grade,
        "n64loaderwv": n64_grade,
        "delta": {
            "matched_exact": n64_grade["matched_exact"] - binary_grade["matched_exact"],
            "interior_entries": n64_grade["interior_entries"] - binary_grade["interior_entries"],
            "wrong": n64_grade["wrong"] - binary_grade["wrong"],
            "open": n64_grade["open"] - binary_grade["open"],
        },
        "answer_key_used_for_grading_only": True,
        "production_ingest_performed": False,
    }
    output = Path(arguments[5])
    if not output.is_absolute() or output.is_symlink() or output.exists() or not output.parent.is_dir():
        fail("OUT must be an absent absolute path under an existing directory")
    output.write_bytes(canonical_bytes(report))


if __name__ == "__main__":
    main(sys.argv)
