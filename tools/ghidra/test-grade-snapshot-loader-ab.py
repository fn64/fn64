#!/usr/bin/env python3

from __future__ import annotations

import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
GRADER = ROOT / "tools/ghidra/grade-snapshot-loader-ab.py"


def canonical(value: object) -> bytes:
    return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()


class LoaderGradeTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="fn64-loader-grade-")
        self.root = Path(self.temporary.name).resolve()
        self.rom = bytearray(0x101000)
        self.rom[:4] = bytes.fromhex("80371240")
        self.bank = bytearray(0x100000)
        # jal 0x80000428; nop -- proves the finer interior root is callable.
        self.bank[0x10:0x14] = (0x0C00010A).to_bytes(4, "big")
        self.rom[0x1000:0x101000] = self.bank
        self.rom_path = self.root / "game.z64"
        self.bank_path = self.root / "bank.bin"
        self.dump_path = self.root / "dump.toml"
        self.rom_path.write_bytes(self.rom)
        self.bank_path.write_bytes(self.bank)
        self.dump_path.write_text(
            """
[[section]]
name = "boot"
rom = 0x1000
vram = 0x80000400
size = 0x60
functions = [
  { name = "entry", vram = 0x80000400, size = 0x20 },
  { name = "coarse", vram = 0x80000420, size = 0x20 },
  { name = "last", vram = 0x80000440, size = 0x20 },
]
""".lstrip(),
            encoding="utf-8",
        )
        self.comparison_path = self.root / "comparison.json"
        self.write_comparison()

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def write_comparison(self) -> None:
        input_value = {
            "normalized_rom_sha256": hashlib.sha256(self.rom).hexdigest(),
            "bank": "boot",
            "bank_bytes_sha256": hashlib.sha256(self.bank).hexdigest(),
            "context_bytes_sha256": "11" * 32,
            "mapping_sha256": "22" * 32,
            "va_start": 0x80000400,
            "va_end": 0x80100400,
            "context_start": 0x80000000,
            "context_end": 0x80400000,
        }
        metrics = {
            "pre_analysis": {},
            "post_analysis": {
                "common_entries": [0x80000420, 0x80000428],
                "binary_only_entries": [],
                "n64loaderwv_only_entries": [0x80000400],
                "binary_entry_count": 2,
                "n64loaderwv_entry_count": 3,
            },
            "memory_map_equal": False,
        }
        inventory = {"fixture": "33" * 32}
        maps = {"binary": "44" * 32, "n64loaderwv": "55" * 32}
        semantic = {
            "schema": "fn64.ghidra-loader-ab",
            "schema_version": 1,
            "input": input_value,
            "inventory_sha256": inventory,
            "memory_map_sha256": maps,
            "metrics": metrics,
        }
        value = {
            "schema": "fn64.ghidra-loader-ab",
            "schema_version": 1,
            "role": "differential_comparison",
            "authority": "candidate_only",
            "context": "shared_mapped_bytes",
            "input": input_value,
            "lane_provenance": {"binary_loader_sha256": "66" * 32, "n64loaderwv_sha256": "77" * 32},
            "inventory_sha256": inventory,
            "memory_map_sha256": maps,
            "metrics": metrics,
            "semantic_sha256": hashlib.sha256(canonical(semantic)).hexdigest(),
        }
        self.comparison_path.write_bytes(canonical(value))

    def invoke(self) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [
                sys.executable,
                str(GRADER),
                str(self.comparison_path),
                str(self.bank_path),
                str(self.rom_path),
                str(self.dump_path),
                str(self.root / "grade.json"),
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )

    def test_grades_vw_gain_without_relabeling_jal_target_as_wrong(self) -> None:
        result = self.invoke()
        self.assertEqual(result.returncode, 0, result.stderr)
        report = json.loads((self.root / "grade.json").read_text())
        self.assertEqual(report["binary_loader"]["matched_exact"], 0)
        self.assertEqual(report["n64loaderwv"]["matched_exact"], 1)
        self.assertEqual(report["binary_loader"]["interior_entries"], 1)
        self.assertEqual(report["n64loaderwv"]["wrong"], 0)
        self.assertEqual(report["delta"], {
            "matched_exact": 1,
            "interior_entries": 0,
            "wrong": 0,
            "open": -1,
        })
        self.assertFalse(report["production_ingest_performed"])

    def test_rejects_semantic_tamper_and_wrong_bank_bytes(self) -> None:
        value = json.loads(self.comparison_path.read_text())
        value["metrics"]["post_analysis"]["n64loaderwv_only_entries"] = []
        self.comparison_path.write_bytes(canonical(value))
        self.assertNotEqual(self.invoke().returncode, 0)

        self.write_comparison()
        self.bank_path.write_bytes(bytes(self.bank) + b"changed")
        self.assertNotEqual(self.invoke().returncode, 0)


if __name__ == "__main__":
    unittest.main()
