#!/usr/bin/env python3

from __future__ import annotations

import hashlib
import json
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
COMPARATOR = ROOT / "tools/ghidra/compare-snapshot-loader-ab.py"
SHA = "12" * 32


def inventory_digest(functions: list[dict[str, object]]) -> str:
    digest = hashlib.sha256(b"fn64.ghidra-bank-function-inventory.v1\0")
    digest.update(struct.pack("<Q", len(functions)))
    for function in functions:
        digest.update(struct.pack("<I", function["entry"]))
        ranges = function["body_ranges"]
        digest.update(struct.pack("<Q", len(ranges)))
        for body_range in ranges:
            digest.update(struct.pack("<II", body_range["va_start"], body_range["va_end"]))
    return digest.hexdigest()


def entry_points_digest(entry_points: list[int]) -> str:
    digest = hashlib.sha256(b"fn64.ghidra-bank-entry-points.v1\0")
    digest.update(struct.pack("<Q", len(entry_points)))
    for entry_point in entry_points:
        digest.update(struct.pack("<I", entry_point))
    return digest.hexdigest()


def rejected_functions_digest(rejected: list[tuple[int, str]]) -> str:
    digest = hashlib.sha256(b"fn64.ghidra-bank-rejected-functions.v1\0")
    digest.update(struct.pack("<Q", len(rejected)))
    for entry, reason in rejected:
        encoded = reason.encode()
        digest.update(struct.pack("<I", entry))
        digest.update(struct.pack("<Q", len(encoded)))
        digest.update(encoded)
    return digest.hexdigest()


class ComparatorTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="fn64-loader-ab-compare-")
        self.root = Path(self.temporary.name).resolve()

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def value(
        self,
        lane: str,
        phase: str,
        functions: list[tuple[int, list[tuple[int, int]]]],
        *,
        source_sha: str = SHA,
        context_sha: str = "34" * 32,
        entry_points: list[int] | None = None,
        rejected: list[tuple[int, str]] | None = None,
    ) -> dict[str, object]:
        entry_points = [] if entry_points is None else entry_points
        rejected = [] if rejected is None else rejected
        function_values = [
            {
                "entry": entry,
                "body_ranges": [
                    {"va_start": start, "va_end": end} for start, end in ranges
                ],
            }
            for entry, ranges in functions
        ]
        return {
            "schema": "fn64.ghidra-bank-function-inventory",
            "schema_version": 4,
            "candidate_only": True,
            "provenance": {"lane": lane, "phase": phase, "source_sha256": source_sha},
            "input": {
                "normalized_rom_sha256": "56" * 32,
                "bank": "bank-a",
                "bank_bytes_sha256": "78" * 32,
                "context_bytes_sha256": context_sha,
                "mapping_sha256": "9a" * 32,
                "va_start": 0x80001000,
                "va_end": 0x80001100,
                "context_start": 0x80000000,
                "context_end": 0x80400000,
            },
            "memory_blocks": [
                {
                    "va_start": 0x80000000,
                    "va_end": 0x80400000,
                    "overlap_start": 0x80000000,
                    "overlap_end": 0x80400000,
                    "read": True,
                    "write": True,
                    "execute": True,
                    "initialized": True,
                }
            ],
            "entry_point_count": len(entry_points),
            "entry_points_sha256": entry_points_digest(entry_points),
            "entry_points": entry_points,
            "rejected_function_count": len(rejected),
            "rejected_functions_sha256": rejected_functions_digest(rejected),
            "rejected_functions": [
                {"entry": entry, "reason": reason} for entry, reason in rejected
            ],
            "function_count": len(function_values),
            "function_inventory_sha256": inventory_digest(function_values),
            "functions": function_values,
        }

    def write(self, name: str, value: object) -> Path:
        path = self.root / name
        path.write_text(json.dumps(value, separators=(",", ":")) + "\n", encoding="utf-8")
        return path

    def invoke(self, values: list[dict[str, object]]) -> subprocess.CompletedProcess[str]:
        inputs = [self.write(f"input-{index}.json", value) for index, value in enumerate(values)]
        return subprocess.run(
            [sys.executable, str(COMPARATOR), *(str(path) for path in inputs), str(self.root / "out.json")],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )

    def four(
        self,
        binary_pre: list[tuple[int, list[tuple[int, int]]]],
        binary_post: list[tuple[int, list[tuple[int, int]]]],
        n64_pre: list[tuple[int, list[tuple[int, int]]]],
        n64_post: list[tuple[int, list[tuple[int, int]]]],
    ) -> list[dict[str, object]]:
        return [
            self.value("binary-loader", "pre", binary_pre, source_sha="01" * 32),
            self.value("binary-loader", "post", binary_post, source_sha="01" * 32),
            self.value("n64loaderwv", "pre", n64_pre, source_sha="02" * 32),
            self.value("n64loaderwv", "post", n64_post, source_sha="02" * 32),
        ]

    def test_exact_agreement(self) -> None:
        functions = [(0x80001000, [(0x80001000, 0x80001010)])]
        result = self.invoke(self.four([], functions, [], functions))
        self.assertEqual(result.returncode, 0, result.stderr)
        report = json.loads((self.root / "out.json").read_text())
        post = report["metrics"]["post_analysis"]
        self.assertEqual(post["common_entries"], [0x80001000])
        self.assertEqual(post["exact_body_entries"], [0x80001000])
        self.assertEqual(post["body_words"]["intersection"], 4)
        self.assertEqual(post["body_words"]["union"], 4)

    def test_reports_seed_unique_entry_and_discontiguous_body_differences(self) -> None:
        binary = [
            (0x80001000, [(0x80001000, 0x80001010)]),
            (0x80001040, [(0x80001040, 0x80001048), (0x80001050, 0x80001058)]),
        ]
        n64 = [
            (0x80001000, [(0x80001000, 0x80001014)]),
            (0x80001020, [(0x80001020, 0x80001028)]),
        ]
        result = self.invoke(self.four([], binary, [(0x80001020, [(0x80001020, 0x80001024)])], n64))
        self.assertEqual(result.returncode, 0, result.stderr)
        report = json.loads((self.root / "out.json").read_text())
        self.assertEqual(
            report["metrics"]["pre_analysis"]["n64loaderwv_only_entries"],
            [0x80001020],
        )
        post = report["metrics"]["post_analysis"]
        self.assertEqual(post["binary_only_entries"], [0x80001040])
        self.assertEqual(post["n64loaderwv_only_entries"], [0x80001020])
        self.assertEqual(post["differing_body_entries"], [0x80001000])
        self.assertGreater(post["body_words"]["binary_only"], 0)
        self.assertGreater(post["body_words"]["n64loaderwv_only"], 0)

    def test_pre_analysis_preserves_body_differences(self) -> None:
        binary_pre = [(0x80001000, [(0x80001000, 0x80001008)])]
        n64_pre = [(0x80001000, [(0x80001000, 0x80001010)])]
        result = self.invoke(self.four(binary_pre, [], n64_pre, []))
        self.assertEqual(result.returncode, 0, result.stderr)
        pre = json.loads((self.root / "out.json").read_text())["metrics"]["pre_analysis"]
        self.assertEqual(pre["differing_body_entries"], [0x80001000])
        self.assertEqual(pre["body_words"]["n64loaderwv_only"], 2)

    def test_pre_analysis_reports_loader_entry_points(self) -> None:
        values = self.four([], [], [], [])
        for index in (2, 3):
            values[index]["entry_point_count"] = 1
            values[index]["entry_points"] = [0x80001000]
            values[index]["entry_points_sha256"] = entry_points_digest([0x80001000])
        result = self.invoke(values)
        self.assertEqual(result.returncode, 0, result.stderr)
        pre = json.loads((self.root / "out.json").read_text())["metrics"]["pre_analysis"]
        self.assertEqual(pre["entry_points"]["n64loaderwv_only"], [0x80001000])

    def test_post_analysis_reports_rejected_non_word_functions(self) -> None:
        values = self.four([], [], [], [])
        values[1] = self.value(
            "binary-loader",
            "post",
            [],
            source_sha="01" * 32,
            rejected=[(0x80001040, "non_word_body_range")],
        )
        values[3] = self.value(
            "n64loaderwv",
            "post",
            [],
            source_sha="02" * 32,
            rejected=[(0x80001040, "non_word_body_range")],
        )
        result = self.invoke(values)
        self.assertEqual(result.returncode, 0, result.stderr)
        report = json.loads((self.root / "out.json").read_text())
        rejected = report["metrics"]["post_analysis"]["rejected_functions"]
        self.assertEqual(rejected["common"], [0x80001040])

    def test_rejects_out_of_bank_body(self) -> None:
        values = self.four([], [(0x80001000, [(0x80001000, 0x80001104)])], [], [])
        result = self.invoke(values)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.root / "out.json").exists())

    def test_rejects_context_or_lane_provenance_drift(self) -> None:
        values = self.four([], [], [], [])
        values[3]["input"]["context_bytes_sha256"] = "ff" * 32
        result = self.invoke(values)
        self.assertNotEqual(result.returncode, 0)
        values = self.four([], [], [], [])
        values[1]["provenance"]["source_sha256"] = "ff" * 32
        result = self.invoke(values)
        self.assertNotEqual(result.returncode, 0)

    def test_rejects_changed_memory_map_within_lane(self) -> None:
        values = self.four([], [], [], [])
        values[1]["memory_blocks"][0]["execute"] = False
        result = self.invoke(values)
        self.assertNotEqual(result.returncode, 0)

    def test_semantic_digest_binds_exact_body_geometry(self) -> None:
        first = self.four([], [(0x80001000, [(0x80001000, 0x80001010)])], [], [])
        result = self.invoke(first)
        self.assertEqual(result.returncode, 0, result.stderr)
        first_digest = json.loads((self.root / "out.json").read_text())["semantic_sha256"]
        (self.root / "out.json").unlink()
        second = self.four([], [(0x80001000, [(0x80001000, 0x80001008), (0x80001010, 0x80001018)])], [], [])
        result = self.invoke(second)
        self.assertEqual(result.returncode, 0, result.stderr)
        second_digest = json.loads((self.root / "out.json").read_text())["semantic_sha256"]
        self.assertNotEqual(first_digest, second_digest)

    def test_rejects_path_like_bank_float_schema_and_digest_tampering(self) -> None:
        for mutation in ("bank", "dot-bank", "dotdot-bank", "schema", "digest"):
            with self.subTest(mutation=mutation):
                values = self.four([], [], [], [])
                if mutation == "bank":
                    for value in values:
                        value["input"]["bank"] = str(self.root / "private-bank")
                elif mutation == "dot-bank":
                    for value in values:
                        value["input"]["bank"] = "."
                elif mutation == "dotdot-bank":
                    for value in values:
                        value["input"]["bank"] = ".."
                elif mutation == "schema":
                    values[0]["schema_version"] = 4.0
                else:
                    values[0]["function_inventory_sha256"] = "ff" * 32
                result = self.invoke(values)
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse((self.root / "out.json").exists())

    def test_exclusive_full_u32_block_end_and_rejections(self) -> None:
        values = self.four([], [], [], [])
        for value in values:
            value["memory_blocks"][0]["va_start"] = 0
            value["memory_blocks"][0]["va_end"] = 0x1_0000_0000
        result = self.invoke(values)
        self.assertEqual(result.returncode, 0, result.stderr)
        for invalid in (0x1_0000_0001, float(0x1_0000_0000)):
            (self.root / "out.json").unlink(missing_ok=True)
            values = self.four([], [], [], [])
            values[0]["memory_blocks"][0]["va_start"] = 0
            values[0]["memory_blocks"][0]["va_end"] = invalid
            result = self.invoke(values)
            self.assertNotEqual(result.returncode, 0)

    def test_semantic_digest_binds_lane_memory_maps(self) -> None:
        values = self.four([], [], [], [])
        values[2]["memory_blocks"][0]["execute"] = False
        values[3]["memory_blocks"][0]["execute"] = False
        result = self.invoke(values)
        self.assertEqual(result.returncode, 0, result.stderr)
        first = json.loads((self.root / "out.json").read_text())
        self.assertFalse(first["metrics"]["memory_map_equal"])
        (self.root / "out.json").unlink()
        values = self.four([], [], [], [])
        values[2]["memory_blocks"][0]["write"] = False
        values[3]["memory_blocks"][0]["write"] = False
        result = self.invoke(values)
        self.assertEqual(result.returncode, 0, result.stderr)
        second = json.loads((self.root / "out.json").read_text())
        self.assertFalse(second["metrics"]["memory_map_equal"])
        self.assertNotEqual(first["memory_map_sha256"], second["memory_map_sha256"])
        self.assertNotEqual(first["semantic_sha256"], second["semantic_sha256"])


if __name__ == "__main__":
    unittest.main()
