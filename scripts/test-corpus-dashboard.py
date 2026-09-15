#!/usr/bin/env python3
"""ROM-free fixture tests for corpus-dashboard.py."""

from __future__ import annotations

import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("corpus-dashboard.py")
SPEC = importlib.util.spec_from_file_location("corpus_dashboard", SCRIPT)
assert SPEC and SPEC.loader
DASHBOARD = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = DASHBOARD
SPEC.loader.exec_module(DASHBOARD)
SHA = "a" * 64


def manifest() -> dict:
    return {"schema": DASHBOARD.MANIFEST_SCHEMA, "campaign_id": "pilot", "roms": [{"id": "one", "normalized_sha256": SHA}]}


def receipt(stage: str, when: str, kind: str = "passed", predecessors: list[str] | None = None, result: dict | None = None) -> dict:
    value = {"schema": DASHBOARD.RECEIPT_SCHEMA, "receipt_id": f"{stage}-{when}", "campaign_id": "pilot", "attempt_id": stage, "stage": stage, "rom": {"id": "one", "normalized_sha256": SHA}, "predecessors": predecessors or [], "outcome": {"kind": kind, "frontier": None}, "result": result if result is not None else {}, "finished_at": when}
    if kind == "frontier":
        value["outcome"]["frontier"] = {"kind": "open_indirect"}
    return value


class DashboardTests(unittest.TestCase):
    def test_contiguous_passes_and_frontier_are_derived(self) -> None:
        report = DASHBOARD.derive(manifest(), [receipt("discover", "1"), receipt("pack", "2", predecessors=["discover-1"]), receipt("recompile", "3", "frontier", ["pack-2"])])
        row = report["rows"][0]
        self.assertEqual(row["highest_passed_stage"], "pack")
        self.assertEqual(row["blocked_at"], "recompile")
        self.assertEqual(row["status"], "blocked_at_recompile")
        self.assertEqual(report["frontier_counts"], {"open_indirect": 1})
        self.assertEqual(report["stage_pass_counts"]["discover"], 1)
        self.assertEqual(report["stage_pass_counts"]["pack"], 1)

    def test_missing_selected_predecessor_is_loud(self) -> None:
        with self.assertRaises(DASHBOARD.DashboardError):
            DASHBOARD.derive(manifest(), [receipt("discover", "1"), receipt("pack", "2")])

    def test_unattempted_stage_is_awaiting_not_blocked(self) -> None:
        report = DASHBOARD.derive(manifest(), [receipt("discover", "1")])
        row = report["rows"][0]
        self.assertIsNone(row["blocked_at"])
        self.assertEqual(row["status"], "awaiting_pack")

    def test_ambiguous_newest_attempt_is_loud(self) -> None:
        with self.assertRaises(DASHBOARD.DashboardError):
            DASHBOARD.derive(manifest(), [receipt("discover", "1"), receipt("discover", "1")])

    def test_coverage_ratios_computed_from_receipts(self) -> None:
        report = DASHBOARD.derive(
            manifest(),
            [
                receipt("discover", "1", result={"code_run_bytes": 1000}),
                receipt("pack", "2", predecessors=["discover-1"]),
                receipt(
                    "recompile", "3", predecessors=["pack-2"],
                    result={"pack_words": 100, "exact_aot_bytes": 150, "block_aot_bytes": 50},
                ),
            ],
        )
        row = report["rows"][0]
        # mapped = pack_words*4 / code_run_bytes = 400/1000 = 0.4
        self.assertEqual(row["coverage"]["mapped_ratio"], {"ratio": 0.4, "status": "ok"})
        # recompiled = (150+50)/1000 = 0.2
        self.assertEqual(row["coverage"]["recompiled_ratio"], {"ratio": 0.2, "status": "ok"})

    def test_coverage_not_run_without_recompile_receipt(self) -> None:
        report = DASHBOARD.derive(
            manifest(),
            [receipt("discover", "1", result={"code_run_bytes": 1000})],
        )
        row = report["rows"][0]
        self.assertEqual(row["coverage"]["mapped_ratio"], {"ratio": None, "status": "not_run"})
        self.assertEqual(row["coverage"]["recompiled_ratio"], {"ratio": None, "status": "not_run"})

    def test_coverage_undefined_when_code_run_bytes_zero_or_missing(self) -> None:
        report_zero = DASHBOARD.derive(
            manifest(),
            [
                receipt("discover", "1", result={"code_run_bytes": 0}),
                receipt("pack", "2", predecessors=["discover-1"]),
                receipt(
                    "recompile", "3", predecessors=["pack-2"],
                    result={"pack_words": 100, "exact_aot_bytes": 150, "block_aot_bytes": 50},
                ),
            ],
        )
        row_zero = report_zero["rows"][0]
        self.assertEqual(row_zero["coverage"]["mapped_ratio"], {"ratio": None, "status": "undefined"})
        self.assertEqual(row_zero["coverage"]["recompiled_ratio"], {"ratio": None, "status": "undefined"})

        # Old discover receipts predate K24 and carry no code_run_bytes at all.
        report_missing = DASHBOARD.derive(
            manifest(),
            [
                receipt("discover", "1"),
                receipt("pack", "2", predecessors=["discover-1"]),
                receipt(
                    "recompile", "3", predecessors=["pack-2"],
                    result={"pack_words": 100, "exact_aot_bytes": 150, "block_aot_bytes": 50},
                ),
            ],
        )
        row_missing = report_missing["rows"][0]
        self.assertEqual(row_missing["coverage"]["mapped_ratio"], {"ratio": None, "status": "undefined"})
        self.assertEqual(row_missing["coverage"]["recompiled_ratio"], {"ratio": None, "status": "undefined"})

    def test_cli_writes_content_free_dashboard(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            manifest_path = root / "manifest.json"
            receipts = root / "receipts"
            receipts.mkdir()
            manifest_path.write_text(json.dumps(manifest()))
            (receipts / "discover.json").write_text(json.dumps(receipt("discover", "1")))
            output_json, output_html = root / "dashboard.json", root / "dashboard.html"
            self.assertEqual(DASHBOARD.main(["--manifest", str(manifest_path), "--receipts", str(receipts), "--output-json", str(output_json), "--output-html", str(output_html)]), 0)
            self.assertEqual(json.loads(output_json.read_text())["rom_count"], 1)
            self.assertIn("highest passed stage", output_html.read_text())
            self.assertIn("Stage progress", output_html.read_text())

    def test_html_contains_coverage_percentages(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            manifest_path = root / "manifest.json"
            receipts = root / "receipts"
            receipts.mkdir()
            manifest_path.write_text(json.dumps(manifest()))
            (receipts / "discover.json").write_text(json.dumps(receipt("discover", "1", result={"code_run_bytes": 1000})))
            (receipts / "pack.json").write_text(json.dumps(receipt("pack", "2", predecessors=["discover-1"])))
            (receipts / "recompile.json").write_text(json.dumps(receipt(
                "recompile", "3", predecessors=["pack-2"],
                result={"pack_words": 100, "exact_aot_bytes": 150, "block_aot_bytes": 50},
            )))
            output_json, output_html = root / "dashboard.json", root / "dashboard.html"
            self.assertEqual(DASHBOARD.main(["--manifest", str(manifest_path), "--receipts", str(receipts), "--output-json", str(output_json), "--output-html", str(output_html)]), 0)
            html_text = output_html.read_text()
            self.assertIn("40.0%", html_text)
            self.assertIn("20.0%", html_text)
            self.assertIn("mapped", html_text)
            self.assertIn("recompiled", html_text)

    def test_html_shows_undefined_for_missing_code_run_bytes(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            manifest_path = root / "manifest.json"
            receipts = root / "receipts"
            receipts.mkdir()
            manifest_path.write_text(json.dumps(manifest()))
            (receipts / "discover.json").write_text(json.dumps(receipt("discover", "1")))
            output_json, output_html = root / "dashboard.json", root / "dashboard.html"
            self.assertEqual(DASHBOARD.main(["--manifest", str(manifest_path), "--receipts", str(receipts), "--output-json", str(output_json), "--output-html", str(output_html)]), 0)
            self.assertIn("undefined", output_html.read_text())


if __name__ == "__main__":
    unittest.main()
