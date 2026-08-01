#!/usr/bin/env python3
"""ROM-free regression tests for rom-frontier.py."""

from __future__ import annotations

import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().with_name("rom-frontier.py")
SPEC = importlib.util.spec_from_file_location("rom_frontier", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
FRONTIER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = FRONTIER
SPEC.loader.exec_module(FRONTIER)


def catalog_record(**overrides: object) -> dict:
    record = {
        "schema": FRONTIER.CATALOG_SCHEMA,
        "normalized_rom_sha256": "a" * 64,
        "stable_id": "test-rom",
        "internal_name": "TEST",
        "ipl3_group": "cic_6102_7101",
        "distinct_jal_targets": 1000,
        "loader_stub_ratio": 0.8,
        "code_run_share": 0.95,
        "boot_entropy": 5.6,
        "unaligned_mem": 10,
        "cache_ops": 2,
        "branch_likely": 30,
    }
    record.update(overrides)
    return record


def summary(digest: str = "a" * 64, **overrides: object) -> dict:
    states = {"Proven": 1, "Candidate": 500, "Supported": 20}
    coverage = {
        "mapped_banks": 1,
        "executable_bytes": 0,
        "function_entries_by_state": states,
    }
    record = {
        "normalized_rom_sha256": digest,
        "selected_strategy": "boot_bank_only",
        "coverage": coverage,
    }
    record.update(overrides)
    return record


class ClassifyTests(unittest.TestCase):
    def test_compressed_boot_wins_over_every_other_signal(self) -> None:
        # A packed boot image is a decompression problem, not a geometry
        # problem, however its other measures read.
        record = catalog_record(boot_entropy=7.86, code_run_share=0.005)
        self.assertEqual(FRONTIER.classify(record), "compressed_boot")

    def test_loader_stub_is_separated_from_resident_code(self) -> None:
        self.assertEqual(
            FRONTIER.classify(catalog_record(loader_stub_ratio=49.9)), "loader_stub"
        )
        self.assertEqual(
            FRONTIER.classify(catalog_record(loader_stub_ratio=0.6)), "resident_code"
        )

    def test_sparse_boot_is_not_mistaken_for_compressed(self) -> None:
        # Low code share with LOW entropy is sparse or relocated, which entropy
        # alone would misfile as healthy.
        record = catalog_record(code_run_share=0.064, boot_entropy=5.72)
        self.assertEqual(FRONTIER.classify(record), "sparse_boot")


class JoinTests(unittest.TestCase):
    def test_join_reports_the_proven_target_gap(self) -> None:
        rows = FRONTIER.join([catalog_record()], {"a" * 64: summary()})
        self.assertEqual(len(rows), 1)
        self.assertEqual(rows[0]["proven_entries"], 1)
        self.assertEqual(rows[0]["distinct_jal_targets"], 1000)
        self.assertEqual(rows[0]["proven_target_share"], 0.001)

    def test_unmatched_catalog_rows_are_dropped_not_guessed(self) -> None:
        with self.assertRaises(FRONTIER.FrontierError):
            FRONTIER.join([catalog_record()], {"b" * 64: summary("b" * 64)})

    def test_zero_targets_does_not_divide_by_zero(self) -> None:
        rows = FRONTIER.join(
            [catalog_record(distinct_jal_targets=0)], {"a" * 64: summary()}
        )
        self.assertEqual(rows[0]["proven_target_share"], 0.0)


class CatalogLoadTests(unittest.TestCase):
    def test_wrong_schema_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "catalog.jsonl"
            path.write_text(json.dumps({"schema": "fn64.something-else.v1"}) + "\n")
            with self.assertRaises(FRONTIER.FrontierError):
                FRONTIER.load_catalog(path)

    def test_empty_catalog_is_loud(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "catalog.jsonl"
            path.write_text("\n")
            with self.assertRaises(FRONTIER.FrontierError):
                FRONTIER.load_catalog(path)

    def test_blank_lines_are_tolerated(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "catalog.jsonl"
            path.write_text(json.dumps(catalog_record()) + "\n\n")
            self.assertEqual(len(FRONTIER.load_catalog(path)), 1)


class OutputTests(unittest.TestCase):
    def test_relative_destination_is_refused(self) -> None:
        with self.assertRaises(FRONTIER.FrontierError):
            FRONTIER.validate_output_destination("relative.jsonl")

    def test_publish_writes_canonical_jsonl(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory).resolve() / "frontier.jsonl"
            FRONTIER.publish_records(path, [{"b": 2, "a": 1}])
            self.assertEqual(path.read_bytes(), b'{"a":1,"b":2}\n')


if __name__ == "__main__":
    unittest.main()
