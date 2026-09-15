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


def receipt(stage: str, when: str, kind: str = "passed", predecessors: list[str] | None = None) -> dict:
    value = {"schema": DASHBOARD.RECEIPT_SCHEMA, "receipt_id": f"{stage}-{when}", "campaign_id": "pilot", "attempt_id": stage, "stage": stage, "rom": {"id": "one", "normalized_sha256": SHA}, "predecessors": predecessors or [], "outcome": {"kind": kind, "frontier": None}, "result": {}, "finished_at": when}
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


if __name__ == "__main__":
    unittest.main()
