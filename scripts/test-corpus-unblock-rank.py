#!/usr/bin/env python3
"""ROM-free fixture tests for corpus-unblock-rank.py.

Builds a temp campaign (manifest + receipts), runs the real
corpus-dashboard.py as a subprocess to derive its dashboard JSON exactly the
way the campaign pipeline does, then exercises corpus-unblock-rank.py against
that dashboard -- both as an importable module (for return-value assertions)
and as a subprocess (for exit-code and stdout assertions).
"""

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().with_name("corpus-unblock-rank.py")
DASHBOARD_SCRIPT = Path(__file__).resolve().with_name("corpus-dashboard.py")

SPEC = importlib.util.spec_from_file_location("corpus_unblock_rank", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
RANK = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = RANK
SPEC.loader.exec_module(RANK)

SHA = "a" * 64


def make_manifest(campaign_id: str, rom_ids: list[str]) -> dict:
    return {
        "schema": "fn64.corpus-campaign.v1",
        "campaign_id": campaign_id,
        "roms": [{"id": rom_id, "normalized_sha256": SHA} for rom_id in rom_ids],
    }


def make_receipt(
    campaign_id: str,
    rom_id: str,
    stage: str,
    receipt_id: str,
    *,
    kind: str = "passed",
    frontier: dict | None = None,
    limit: str | None = None,
    predecessors: list[str] | None = None,
    finished_at: str = "2026-09-15T00:00:00Z",
) -> dict:
    outcome: dict = {"kind": kind, "frontier": frontier}
    if limit is not None:
        outcome["limit"] = limit
    return {
        "schema": "fn64.corpus-stage-receipt.v1",
        "receipt_id": receipt_id,
        "campaign_id": campaign_id,
        "attempt_id": f"{stage}-0001",
        "stage": stage,
        "rom": {"id": rom_id, "normalized_sha256": SHA},
        "predecessors": predecessors or [],
        "outcome": outcome,
        "result": {},
        "finished_at": finished_at,
    }


class Campaign:
    """Builds a temp campaign directory and its derived dashboard.json."""

    def __init__(self, root: Path, campaign_id: str = "unblock-rank-pilot") -> None:
        self.root = root
        self.campaign_id = campaign_id
        self.manifest_path = root / "manifest.json"
        self.receipts_dir = root / "receipts"
        self.receipts_dir.mkdir(parents=True, exist_ok=True)
        self.rom_ids: list[str] = []
        self._receipts: list[dict] = []

    def add_rom(
        self,
        rom_id: str,
        *,
        pack_kind: str = "passed",
        pack_frontier: dict | None = None,
        pack_limit: str | None = None,
        recompile_kind: str = "passed",
        recompile_frontier: dict | None = None,
        recompile_limit: str | None = None,
        skip_recompile: bool = False,
    ) -> None:
        self.rom_ids.append(rom_id)
        discover_id = f"discover-{rom_id}"
        pack_id = f"pack-{rom_id}"
        recompile_id = f"recompile-{rom_id}"
        self._receipts.append(make_receipt(self.campaign_id, rom_id, "discover", discover_id))
        self._receipts.append(
            make_receipt(
                self.campaign_id,
                rom_id,
                "pack",
                pack_id,
                kind=pack_kind,
                frontier=pack_frontier,
                limit=pack_limit,
                predecessors=[discover_id],
            )
        )
        if pack_kind != "passed":
            skip_recompile = True
        if not skip_recompile:
            self._receipts.append(
                make_receipt(
                    self.campaign_id,
                    rom_id,
                    "recompile",
                    recompile_id,
                    kind=recompile_kind,
                    frontier=recompile_frontier,
                    limit=recompile_limit,
                    predecessors=[pack_id],
                )
            )

    def finalize(self) -> None:
        self.manifest_path.write_text(json.dumps(make_manifest(self.campaign_id, self.rom_ids)))
        for index, receipt in enumerate(self._receipts):
            (self.receipts_dir / f"r{index:03d}.json").write_text(json.dumps(receipt))

    def build_dashboard(self) -> Path:
        dashboard_json = self.root / "dashboard.json"
        dashboard_html = self.root / "dashboard.html"
        result = subprocess.run(
            [
                sys.executable,
                str(DASHBOARD_SCRIPT),
                "--manifest",
                str(self.manifest_path),
                "--receipts",
                str(self.receipts_dir),
                "--output-json",
                str(dashboard_json),
                "--output-html",
                str(dashboard_html),
            ],
            capture_output=True,
            text=True,
        )
        assert result.returncode == 0, f"corpus-dashboard.py failed: {result.stderr}"
        return dashboard_json


UNSUPPORTED_ONE = {"kind": "unsupported_destinations", "count": 1, "reasons": ["OutsideAllMappings"]}
UNSUPPORTED_FIVE = {
    "kind": "unsupported_destinations",
    "count": 5,
    "reasons": ["OutsideAllMappings", "ProvenCodeNoOwner"],
}
NO_UNIQUE_ADMITTED = {"kind": "NoUniqueAdmittedTable", "detail": "two admitted tables tie"}


def build_reference_campaign(root: Path) -> Path:
    """3 passed, 2x unsupported(count=1), 1x unsupported(count=5), 1x
    NoUniqueAdmittedTable, 1x resource_limit -- 8 ROMs total, matching the
    task's required fixture coverage."""
    campaign = Campaign(root)
    campaign.add_rom("rom-passed-1")
    campaign.add_rom("rom-passed-2")
    campaign.add_rom("rom-passed-3")
    campaign.add_rom("rom-unsup1-a", recompile_kind="frontier", recompile_frontier=UNSUPPORTED_ONE)
    campaign.add_rom("rom-unsup1-b", recompile_kind="frontier", recompile_frontier=UNSUPPORTED_ONE)
    campaign.add_rom("rom-unsup5", recompile_kind="frontier", recompile_frontier=UNSUPPORTED_FIVE)
    campaign.add_rom("rom-nouniq", recompile_kind="frontier", recompile_frontier=NO_UNIQUE_ADMITTED)
    campaign.add_rom("rom-limit", recompile_kind="resource_limit", recompile_frontier=None, recompile_limit="wall_time_ms")
    campaign.finalize()
    return campaign.build_dashboard(), campaign


class UnblockRankTests(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name).resolve()
        self.dashboard_path, self.campaign = build_reference_campaign(self.root)

    def tearDown(self) -> None:
        self._tmp.cleanup()

    def test_row_order_counts_and_reconciliation(self) -> None:
        dashboard = RANK.load_dashboard(self.dashboard_path)
        report = RANK.rank(self.root, dashboard, "recompile")

        frontier_rows = [row for row in report["rows"] if row["section"] == "frontier"]
        other_rows = [row for row in report["rows"] if row["section"] != "frontier"]

        # Descending by ROM count: unsupported(count=1) cluster has 2 ROMs
        # and must rank first; the other frontier clusters have 1 ROM each.
        self.assertEqual(frontier_rows[0]["rom_count"], 2)
        self.assertEqual(sorted(frontier_rows[0]["rom_ids"]), ["rom-unsup1-a", "rom-unsup1-b"])
        self.assertEqual(frontier_rows[0]["key"], ["unsupported_destinations", "1", ["OutsideAllMappings"]])

        remaining_counts = sorted(row["rom_count"] for row in frontier_rows[1:])
        self.assertEqual(remaining_counts, [1, 1])

        unsup5_rows = [row for row in frontier_rows if row["key"][0] == "unsupported_destinations" and row["key"][1] == "4+"]
        self.assertEqual(len(unsup5_rows), 1)
        self.assertEqual(unsup5_rows[0]["rom_ids"], ["rom-unsup5"])
        self.assertEqual(sorted(unsup5_rows[0]["key"][2]), ["OutsideAllMappings", "ProvenCodeNoOwner"])

        nouniq_rows = [row for row in frontier_rows if row["key"] == ["NoUniqueAdmittedTable"]]
        self.assertEqual(len(nouniq_rows), 1)
        self.assertEqual(nouniq_rows[0]["rom_ids"], ["rom-nouniq"])

        # resource_limit is its own row, never folded into a frontier cluster.
        self.assertEqual(len(other_rows), 1)
        self.assertEqual(other_rows[0]["key"], ["resource_limit", "wall_time_ms"])
        self.assertEqual(other_rows[0]["rom_ids"], ["rom-limit"])

        # passed line: denominator is ROMs with a selected recompile receipt.
        self.assertEqual(report["passed"], 3)
        self.assertEqual(report["total"], 8)
        self.assertEqual(report["not_run"], 0)

        # Reconciliation: frontier cluster ROM counts sum to the dashboard's
        # frontier total for this stage (2 + 1 + 1 = 4), matching
        # frontier_counts for a single-blocking-stage campaign.
        frontier_total = sum(row["rom_count"] for row in frontier_rows)
        self.assertEqual(frontier_total, sum(dashboard["frontier_counts"].values()))
        self.assertEqual(frontier_total, 4)

    def test_descending_sort_across_full_row_set(self) -> None:
        dashboard = RANK.load_dashboard(self.dashboard_path)
        report = RANK.rank(self.root, dashboard, "recompile")
        counts = [row["rom_count"] for row in report["rows"] if row["section"] == "frontier"]
        self.assertEqual(counts, sorted(counts, reverse=True))

    def test_not_run_rom_is_counted_separately(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            campaign = Campaign(root)
            campaign.add_rom("rom-passed")
            campaign.add_rom("rom-not-run", skip_recompile=True)
            campaign.finalize()
            dashboard_path = campaign.build_dashboard()
            dashboard = RANK.load_dashboard(dashboard_path)
            report = RANK.rank(root, dashboard, "recompile")
            self.assertEqual(report["passed"], 1)
            self.assertEqual(report["total"], 1)
            self.assertEqual(report["not_run"], 1)

    def test_discover_stage_no_candidate_table_found_is_not_blocked(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            campaign_id = "discover-pilot"
            manifest_path = root / "manifest.json"
            receipts_dir = root / "receipts"
            receipts_dir.mkdir()
            rom_id = "rom-geometry"
            receipt = make_receipt(campaign_id, rom_id, "discover", "discover-1")
            receipt["result"] = {"frontier": {"geometry_failure": "no_candidate_table_found"}}
            manifest_path.write_text(json.dumps(make_manifest(campaign_id, [rom_id])))
            (receipts_dir / "r0.json").write_text(json.dumps(receipt))
            dashboard_json = root / "dashboard.json"
            dashboard_html = root / "dashboard.html"
            result = subprocess.run(
                [
                    sys.executable, str(DASHBOARD_SCRIPT),
                    "--manifest", str(manifest_path), "--receipts", str(receipts_dir),
                    "--output-json", str(dashboard_json), "--output-html", str(dashboard_html),
                ],
                capture_output=True, text=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            dashboard = RANK.load_dashboard(dashboard_json)
            report = RANK.rank(root, dashboard, "discover")
            self.assertEqual(report["passed"], 1)
            self.assertEqual(len(report["rows"]), 0)

    def test_tampered_dashboard_frontier_counts_exits_nonzero(self) -> None:
        tampered = json.loads(self.dashboard_path.read_text())
        tampered["frontier_counts"] = {key: value + 1 for key, value in tampered["frontier_counts"].items()}
        tampered_path = self.root / "tampered-dashboard.json"
        tampered_path.write_text(json.dumps(tampered))

        result = subprocess.run(
            [
                sys.executable, str(SCRIPT),
                "--campaign-dir", str(self.root),
                "--dashboard-json", str(tampered_path),
                "--stage", "recompile",
            ],
            capture_output=True, text=True,
        )
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("reconciliation failed", result.stderr)

    def test_cli_runs_clean_on_reference_campaign(self) -> None:
        result = subprocess.run(
            [
                sys.executable, str(SCRIPT),
                "--campaign-dir", str(self.root),
                "--dashboard-json", str(self.dashboard_path),
                "--stage", "recompile",
            ],
            capture_output=True, text=True,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("passed: 3 of 8, not_run: 0", result.stdout)
        self.assertIn("resource-limit / infrastructure-failure", result.stdout)

    def test_json_output_matches_in_memory_report(self) -> None:
        json_out = self.root / "unblock-rank.json"
        result = subprocess.run(
            [
                sys.executable, str(SCRIPT),
                "--campaign-dir", str(self.root),
                "--dashboard-json", str(self.dashboard_path),
                "--stage", "recompile",
                "--json", str(json_out),
            ],
            capture_output=True, text=True,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        written = json.loads(json_out.read_text())
        dashboard = RANK.load_dashboard(self.dashboard_path)
        expected = RANK.rank(self.root, dashboard, "recompile")
        self.assertEqual(written, expected)

    def test_no_absolute_path_in_stdout(self) -> None:
        json_out = self.root / "unblock-rank.json"
        result = subprocess.run(
            [
                sys.executable, str(SCRIPT),
                "--campaign-dir", str(self.root),
                "--dashboard-json", str(self.dashboard_path),
                "--stage", "recompile",
                "--json", str(json_out),
            ],
            capture_output=True, text=True,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertNotIn(str(self.root), result.stdout)
        self.assertNotIn(str(self.root.parent), result.stdout)
        written = json_out.read_text()
        self.assertNotIn(str(self.root), written)
        self.assertNotIn(str(self.root.parent), written)

    def test_malformed_unsupported_destinations_is_loud(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            campaign = Campaign(root)
            campaign.add_rom(
                "rom-bad",
                recompile_kind="frontier",
                recompile_frontier={"kind": "unsupported_destinations", "count": "not-a-number", "reasons": []},
            )
            campaign.finalize()
            dashboard_path = campaign.build_dashboard()
            dashboard = RANK.load_dashboard(dashboard_path)
            with self.assertRaises(RANK.UnblockRankError):
                RANK.rank(root, dashboard, "recompile")

    def test_unknown_stage_is_rejected(self) -> None:
        dashboard = RANK.load_dashboard(self.dashboard_path)
        with self.assertRaises(RANK.UnblockRankError):
            RANK.rank(self.root, dashboard, "not-a-stage")


PACK_INVALID_RESIDENT_SPLIT = {"kind": "InvalidResidentSplit", "detail": "invalid generation topology"}
PACK_NO_UNIQUE_ADMITTED = {"kind": "NoUniqueAdmittedTable", "phase": "recovering complete overlay load recipes", "detail": "NoUniqueAdmittedTable { admitted: 2 }"}
PACK_INVALID_RANGE_RELATIONS = {"kind": "InvalidRangeRelations", "phase": "recovering complete overlay load recipes", "detail": "InvalidRangeRelations { record: 0 }"}


def build_pack_reference_campaign(root: Path):
    """--stage pack coverage: 2 passed (through recompile), 3 distinct pack
    frontier kinds (one ROM each, so (kind,) clustering, not (kind, phase)),
    1 pack resource_limit. ROMs whose pack is not `passed` get no recompile
    receipt, matching corpus-recompile-sweep.py's real behavior."""
    campaign = Campaign(root, campaign_id="unblock-rank-pack-pilot")
    campaign.add_rom("rom-passed-1")
    campaign.add_rom("rom-passed-2")
    campaign.add_rom("rom-pack-split", pack_kind="frontier", pack_frontier=PACK_INVALID_RESIDENT_SPLIT)
    campaign.add_rom("rom-pack-nouniq-a", pack_kind="frontier", pack_frontier=PACK_NO_UNIQUE_ADMITTED)
    campaign.add_rom("rom-pack-nouniq-b", pack_kind="frontier", pack_frontier=PACK_NO_UNIQUE_ADMITTED)
    campaign.add_rom("rom-pack-range", pack_kind="frontier", pack_frontier=PACK_INVALID_RANGE_RELATIONS)
    campaign.add_rom("rom-pack-limit", pack_kind="resource_limit", pack_limit="wall_time_ms")
    campaign.finalize()
    return campaign.build_dashboard(), campaign


class PackStageTests(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name).resolve()
        self.dashboard_path, self.campaign = build_pack_reference_campaign(self.root)

    def tearDown(self) -> None:
        self._tmp.cleanup()

    def test_pack_stage_accepted_and_clusters_by_kind_only(self) -> None:
        dashboard = RANK.load_dashboard(self.dashboard_path)
        report = RANK.rank(self.root, dashboard, "pack")
        self.assertEqual(report["stage"], "pack")

        frontier_rows = [row for row in report["rows"] if row["section"] == "frontier"]
        other_rows = [row for row in report["rows"] if row["section"] != "frontier"]

        # Two ROMs share the same (kind,) -- not (kind, phase) -- despite
        # both carrying identical phase text; they must cluster together
        # under a bare ["NoUniqueAdmittedTable"] key.
        nouniq_rows = [row for row in frontier_rows if row["key"] == ["NoUniqueAdmittedTable"]]
        self.assertEqual(len(nouniq_rows), 1)
        self.assertEqual(sorted(nouniq_rows[0]["rom_ids"]), ["rom-pack-nouniq-a", "rom-pack-nouniq-b"])
        self.assertEqual(nouniq_rows[0]["rom_count"], 2)

        split_rows = [row for row in frontier_rows if row["key"] == ["InvalidResidentSplit"]]
        self.assertEqual(len(split_rows), 1)
        self.assertEqual(split_rows[0]["rom_ids"], ["rom-pack-split"])

        range_rows = [row for row in frontier_rows if row["key"] == ["InvalidRangeRelations"]]
        self.assertEqual(len(range_rows), 1)
        self.assertEqual(range_rows[0]["rom_ids"], ["rom-pack-range"])

        self.assertEqual(len(other_rows), 1)
        self.assertEqual(other_rows[0]["key"], ["resource_limit", "wall_time_ms"])
        self.assertEqual(other_rows[0]["rom_ids"], ["rom-pack-limit"])

        self.assertEqual(report["passed"], 2)
        self.assertEqual(report["total"], 7)
        self.assertEqual(report["not_run"], 0)

    def test_pack_stage_reconciles_and_cli_runs_clean(self) -> None:
        result = subprocess.run(
            [
                sys.executable, str(SCRIPT),
                "--campaign-dir", str(self.root),
                "--dashboard-json", str(self.dashboard_path),
                "--stage", "pack",
            ],
            capture_output=True, text=True,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("stage: pack", result.stdout)
        self.assertIn("passed: 2 of 7, not_run: 0", result.stdout)


if __name__ == "__main__":
    unittest.main()
