#!/usr/bin/env python3
"""ROM-free fixture tests for corpus-recompile-sweep.py.

Builds a tiny campaign (manifest + one `passed` discover receipt per ROM,
minted the way import-rom-frontier-campaign.py does) and a fake
`fn64-discover` binary that emits scripted output per ROM name, one ROM per
closed outcome kind: passed, unsupported>0 (frontier), FAILED with no report
(pack frontier, no recompile receipt), timeout (resource_limit), and garbage
output (infrastructure_failure). The gate packs before it emits, so each run
can mint up to two receipts: `pack` (predecessor: discover), then
`recompile` (predecessor: pack) only when pack passed.
"""

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("corpus-recompile-sweep.py")
SPEC = importlib.util.spec_from_file_location("corpus_recompile_sweep", SCRIPT)
assert SPEC and SPEC.loader
SWEEP = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = SWEEP
SPEC.loader.exec_module(SWEEP)

DASHBOARD_SCRIPT = Path(__file__).with_name("corpus-dashboard.py")

RECEIPT_SCHEMA = "fn64.corpus-stage-receipt.v1"
MANIFEST_SCHEMA = "fn64.corpus-campaign.v1"


def sha_for(label: str) -> str:
    import hashlib

    return hashlib.sha256(label.encode()).hexdigest()


ROM_IDS = ["passed-rom", "frontier-rom", "failed-rom", "timeout-rom", "garbage-rom"]


FAKE_BINARY = r'''#!/usr/bin/env python3
import json
import os
import sys
import time

rom = os.environ.get("FN64_DISCOVER_ROM", "")
report_path = os.environ.get("FN64_RECOMPILE_REPORT")

def write_report(unsupported):
    if not report_path:
        return
    with open(report_path, "w") as handle:
        json.dump({
            "schema": "fn64.rom-recompile-report.v1",
            "schema_version": 1,
            "normalized_rom_sha256": "0" * 64,
            "internal_name": "FAKE",
            "banks": 1,
            "pack_blocks": 1,
            "pack_words": 1,
            "emitted_code_bytes": 4,
            "exact_aot_bytes": 4,
            "block_aot_bytes": 0,
            "dynamic_mips_destinations": 0,
            "unsupported_destinations": unsupported,
            "total_destinations": unsupported + 1,
            "runner_sha256": "1" * 64,
            "rustc_compiles": True,
            "harness_runs": True,
        }, handle)

if len(sys.argv) >= 2 and sys.argv[1] == "diagnose-cold-unsupported":
    diag_rom = sys.argv[2] if len(sys.argv) > 2 else ""
    if "frontier-rom" in diag_rom:
        record = {
            "schema": "fn64.cold-unsupported-diagnostic.v1",
            "schema_version": 1,
            "normalized_rom_sha256": "0" * 64,
            "selected_strategy": "recovered_overlays",
            "proven_bank_count": 1,
            "closure": {},
            "unsupported_destinations": [
                {"destination_va": 4096, "reason": "OpenIndirectSite", "incoming": []},
                {"destination_va": 4100, "reason": "MappedNotProvenCode", "incoming": []},
                {"destination_va": 4104, "reason": "OpenIndirectSite", "incoming": []},
            ],
        }
        print(json.dumps(record))
        sys.exit(0)
    sys.exit(1)

if len(sys.argv) >= 2 and sys.argv[1] == "gate-rom-recompile":
    if "passed-rom" in rom:
        write_report(0)
        print("HEADLINE unsupported=0 total_recompiled_exact_plus_block_aot_bytes=4")
        sys.exit(0)
    if "frontier-rom" in rom:
        write_report(3)
        print("HEADLINE unsupported=3 total_recompiled_exact_plus_block_aot_bytes=4")
        sys.exit(1)
    if "failed-rom" in rom:
        sys.stderr.write("gate_rom_recompile: FAILED: NoUniqueAdmittedTable at /private/tmp/some/rom/path.z64\n")
        sys.exit(1)
    if "timeout-rom" in rom:
        time.sleep(30)
        sys.exit(0)
    if "garbage-rom" in rom:
        print("this is not a recognised headline line")
        sys.exit(1)
    sys.exit(1)

sys.exit(1)
'''


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), allow_nan=False).encode()


def build_campaign(root: Path) -> Path:
    campaign_dir = root / "campaign"
    receipts_dir = campaign_dir / "receipts"
    receipts_dir.mkdir(parents=True)
    roms = []
    for rom_id in ROM_IDS:
        digest = sha_for(rom_id)
        roms.append({"id": rom_id, "normalized_sha256": digest})
    manifest = {"schema": MANIFEST_SCHEMA, "campaign_id": "fixture-campaign", "roms": roms}
    (campaign_dir / "manifest.json").write_bytes(canonical(manifest) + b"\n")
    for rom in roms:
        payload = canonical({"rom_id": rom["id"]})
        receipt_id = "frontier-discover-" + SWEEP.sha256_bytes(payload)
        receipt = {
            "schema": RECEIPT_SCHEMA,
            "receipt_id": receipt_id,
            "campaign_id": "fixture-campaign",
            "attempt_id": "frontier-full-fixture",
            "stage": "discover",
            "rom": {"id": rom["id"], "normalized_sha256": rom["normalized_sha256"]},
            "predecessors": [],
            "outcome": {"kind": "passed", "frontier": None},
            "result": {},
            "finished_at": "2026-09-15T00:00:00Z",
        }
        (receipts_dir / f"{receipt_id}.json").write_bytes(canonical(receipt) + b"\n")
    return campaign_dir


def build_rom_map(root: Path, rom_dir: Path) -> Path:
    entries = []
    for rom_id in ROM_IDS:
        rom_path = rom_dir / f"{rom_id}.z64"
        rom_path.write_bytes(rom_id.encode())
        entries.append({"normalized_sha256": sha_for(rom_id), "path": str(rom_path)})
    rom_map_path = root / "rom-map.jsonl"
    rom_map_path.write_text("\n".join(json.dumps(entry) for entry in entries) + "\n")
    return rom_map_path


def build_candidate(root: Path) -> Path:
    candidate = {
        "git_commit": "a" * 40,
        "worktree_sha256": "b" * 64,
        "binary_sha256": "c" * 64,
        "toolchain": "rustc 1.0.0 fixture",
    }
    path = root / "candidate.json"
    path.write_text(json.dumps(candidate))
    return path


def build_fake_binary(root: Path) -> Path:
    path = root / "fake-fn64-discover"
    path.write_text(FAKE_BINARY)
    path.chmod(0o755)
    return path


class RunHelpers:
    def setUp(self) -> None:
        self.tempdir = tempfile.TemporaryDirectory()
        self.root = Path(self.tempdir.name)
        self.campaign_dir = build_campaign(self.root)
        self.rom_dir = self.root / "roms"
        self.rom_dir.mkdir()
        self.rom_map = build_rom_map(self.root, self.rom_dir)
        self.candidate = build_candidate(self.root)
        self.binary = build_fake_binary(self.root)

    def tearDown(self) -> None:
        self.tempdir.cleanup()

    def run_sweep(self, extra: list[str] | None = None) -> subprocess.CompletedProcess:
        argv = [
            sys.executable,
            str(SCRIPT),
            "--campaign-dir",
            str(self.campaign_dir),
            "--binary",
            str(self.binary),
            "--rom-map",
            str(self.rom_map),
            "--candidate",
            str(self.candidate),
            "--timeout-seconds",
            "3",
            "--jobs",
            "1",
        ]
        argv += extra or []
        return subprocess.run(argv, capture_output=True, text=True, timeout=60)

    def receipts_for_stage(self, stage: str) -> list[dict]:
        out = []
        for path in sorted((self.campaign_dir / "receipts").rglob("*.json")):
            value = json.loads(path.read_text())
            if value.get("stage") == stage:
                out.append(value)
        return out


class FailedKindPhaseAndDetailTests(unittest.TestCase):
    """The four real FAILED lines from the campaign transcripts (K19)."""

    def test_no_unique_admitted_table(self) -> None:
        line = (
            "recovering complete overlay load recipes: NoUniqueAdmittedTable { admitted: 2 }; "
            "load-only fallback also failed: NoUniqueAdmittedTable { admitted: 2 }"
        )
        kind, phase, detail = SWEEP.failed_kind_phase_and_detail(line)
        self.assertEqual(kind, "NoUniqueAdmittedTable")
        self.assertEqual(phase, "recovering complete overlay load recipes")
        self.assertIn("NoUniqueAdmittedTable", detail)

    def test_invalid_range_relations(self) -> None:
        line = (
            "recovering complete overlay load recipes: InvalidRangeRelations { record: 0 }; "
            "load-only fallback proved no shared destination slot"
        )
        kind, phase, detail = SWEEP.failed_kind_phase_and_detail(line)
        self.assertEqual(kind, "InvalidRangeRelations")
        self.assertEqual(phase, "recovering complete overlay load recipes")
        self.assertIn("InvalidRangeRelations", detail)

    def test_invalid_resident_split(self) -> None:
        line = "building generation topology: invalid generation topology: InvalidResidentSplit"
        kind, phase, detail = SWEEP.failed_kind_phase_and_detail(line)
        self.assertEqual(kind, "InvalidResidentSplit")
        self.assertEqual(phase, "building generation topology")
        self.assertIn("InvalidResidentSplit", detail)

    def test_unaligned_field(self) -> None:
        line = (
            "recovering complete overlay load recipes: UnalignedField { record: 0, value: 1835111 }; ..."
        )
        kind, phase, detail = SWEEP.failed_kind_phase_and_detail(line)
        self.assertEqual(kind, "UnalignedField")
        self.assertEqual(phase, "recovering complete overlay load recipes")
        self.assertIn("UnalignedField", detail)

    def test_fallback_to_first_token_when_no_camel_case(self) -> None:
        kind, phase, detail = SWEEP.failed_kind_phase_and_detail("timeout waiting for descriptor")
        self.assertEqual(kind, "timeout")
        self.assertIsNone(phase)
        self.assertEqual(detail, "waiting for descriptor")

    def test_strips_paths_from_detail(self) -> None:
        kind, phase, detail = SWEEP.failed_kind_phase_and_detail(
            "NoUniqueAdmittedTable at /private/tmp/some/rom/path.z64"
        )
        self.assertEqual(kind, "NoUniqueAdmittedTable")
        self.assertIsNone(phase)
        self.assertNotIn("/private/tmp", detail)


class OutcomeMappingTests(RunHelpers, unittest.TestCase):
    def test_full_sweep_maps_every_outcome_kind(self) -> None:
        proc = self.run_sweep()
        self.assertEqual(proc.returncode, 0, proc.stderr)
        pack_receipts = {r["rom"]["id"]: r for r in self.receipts_for_stage("pack")}
        recompile_receipts = {r["rom"]["id"]: r for r in self.receipts_for_stage("recompile")}

        # Every ROM gets a pack receipt.
        self.assertEqual(set(pack_receipts), set(ROM_IDS))
        # Only ROMs whose pack passed get a recompile receipt: passed-rom and
        # frontier-rom pack (pack_words=1 in their fake reports); failed-rom
        # produces no report so its pack is a frontier and it gets no
        # recompile receipt; timeout/garbage never reach a report either.
        self.assertEqual(set(recompile_receipts), {"passed-rom", "frontier-rom"})

        # --- pack outcomes --------------------------------------------
        self.assertEqual(pack_receipts["passed-rom"]["outcome"]["kind"], "passed")
        self.assertIsNone(pack_receipts["passed-rom"]["outcome"]["frontier"])
        self.assertEqual(pack_receipts["passed-rom"]["result"]["pack_words"], 1)

        self.assertEqual(pack_receipts["frontier-rom"]["outcome"]["kind"], "passed")
        self.assertEqual(pack_receipts["frontier-rom"]["result"]["pack_words"], 1)

        failed_pack = pack_receipts["failed-rom"]
        self.assertEqual(failed_pack["outcome"]["kind"], "frontier")
        self.assertEqual(failed_pack["outcome"]["frontier"]["kind"], "NoUniqueAdmittedTable")
        self.assertNotIn("phase", failed_pack["outcome"]["frontier"])
        self.assertNotIn("/private/tmp", failed_pack["outcome"]["frontier"]["detail"])
        self.assertNotIn("pack_words", failed_pack["result"])

        timeout_pack = pack_receipts["timeout-rom"]
        self.assertEqual(timeout_pack["outcome"]["kind"], "resource_limit")
        self.assertIsNone(timeout_pack["outcome"]["frontier"])
        self.assertEqual(timeout_pack["result"]["resource_limit"]["which"], "wall_time")

        garbage_pack = pack_receipts["garbage-rom"]
        self.assertEqual(garbage_pack["outcome"]["kind"], "infrastructure_failure")
        self.assertIsNone(garbage_pack["outcome"]["frontier"])

        # --- recompile outcomes (only passed-rom and frontier-rom) ----
        passed = recompile_receipts["passed-rom"]
        self.assertEqual(passed["outcome"]["kind"], "passed")
        self.assertIsNone(passed["outcome"]["frontier"])
        self.assertEqual(passed["result"]["unsupported_destinations"], 0)

        frontier = recompile_receipts["frontier-rom"]
        self.assertEqual(frontier["outcome"]["kind"], "frontier")
        self.assertEqual(frontier["outcome"]["frontier"]["kind"], "unsupported_destinations")
        self.assertEqual(frontier["outcome"]["frontier"]["count"], 3)
        self.assertEqual(
            frontier["outcome"]["frontier"]["reasons"],
            ["MappedNotProvenCode", "OpenIndirectSite"],
        )
        self.assertNotIn("diagnostic_failed", frontier["outcome"]["frontier"])

        # --- K19 fix 2: cold-unsupported JSON retained as a private
        # artifact, and result.unsupported carries address-only summaries.
        artifact_kinds = {a["kind"] for a in frontier["artifacts"]}
        self.assertIn("cold_unsupported", artifact_kinds)
        cold_artifact = next(a for a in frontier["artifacts"] if a["kind"] == "cold_unsupported")
        self.assertEqual(cold_artifact["visibility"], "private")
        artifact_path = (
            self.campaign_dir / "artifacts" / "frontier-rom" / frontier["attempt_id"] / "cold-unsupported.json"
        )
        self.assertTrue(artifact_path.is_file())
        self.assertEqual(SWEEP.sha256_file(artifact_path), cold_artifact["sha256"])
        artifact_record = json.loads(artifact_path.read_text())
        self.assertEqual(artifact_record["schema"], "fn64.cold-unsupported-diagnostic.v1")

        unsupported = frontier["result"]["unsupported"]
        self.assertEqual(len(unsupported), 3)
        self.assertEqual(
            sorted(item["destination_va"] for item in unsupported),
            sorted([hex(4096), hex(4100), hex(4104)]),
        )
        for item in unsupported:
            self.assertIn(item["reason"], ("OpenIndirectSite", "MappedNotProvenCode"))
            self.assertEqual(item["incoming_kinds"], [])
        # Addresses only -- never a path or raw byte value.
        self.assertNotIn(str(self.rom_dir), json.dumps(unsupported))

    def test_receipts_bind_pack_then_recompile_predecessors(self) -> None:
        self.run_sweep()
        discover_by_rom = {r["rom"]["id"]: r["receipt_id"] for r in self.receipts_for_stage("discover")}
        pack_by_rom = {r["rom"]["id"]: r for r in self.receipts_for_stage("pack")}

        for receipt in self.receipts_for_stage("pack"):
            rom_id = receipt["rom"]["id"]
            self.assertEqual(receipt["predecessors"], [discover_by_rom[rom_id]])
            self.assertEqual(receipt["candidate"]["git_commit"], "a" * 40)
            self.assertEqual(receipt["candidate"]["binary_sha256"], "c" * 64)
            self.assertIn("wall_time_ms", receipt["policy"])
            self.assertEqual(receipt["policy"]["timeout_seconds"], 3)
            self.assertEqual(len(receipt["artifacts"]), 1)
            self.assertEqual(receipt["artifacts"][0]["kind"], "stdout")
            self.assertEqual(receipt["artifacts"][0]["visibility"], "private")

        for receipt in self.receipts_for_stage("recompile"):
            rom_id = receipt["rom"]["id"]
            self.assertEqual(receipt["predecessors"], [pack_by_rom[rom_id]["receipt_id"]])

    def test_receipt_id_is_sha256_of_canonical_body(self) -> None:
        self.run_sweep()
        for receipt in self.receipts_for_stage("pack"):
            body = dict(receipt)
            receipt_id = body.pop("receipt_id")
            self.assertEqual(receipt_id, "pack-" + SWEEP.sha256_bytes(SWEEP.canonical_json(body)))
        for receipt in self.receipts_for_stage("recompile"):
            body = dict(receipt)
            receipt_id = body.pop("receipt_id")
            self.assertEqual(receipt_id, "recompile-" + SWEEP.sha256_bytes(SWEEP.canonical_json(body)))


class ValidationTests(RunHelpers, unittest.TestCase):
    def test_receipts_validate_under_corpus_dashboard(self) -> None:
        self.run_sweep()
        with tempfile.TemporaryDirectory() as out_dir:
            out = Path(out_dir).resolve()
            proc = subprocess.run(
                [
                    sys.executable,
                    str(DASHBOARD_SCRIPT),
                    "--manifest",
                    str(self.campaign_dir / "manifest.json"),
                    "--receipts",
                    str(self.campaign_dir / "receipts"),
                    "--output-json",
                    str(out / "dashboard.json"),
                    "--output-html",
                    str(out / "dashboard.html"),
                ],
                capture_output=True,
                text=True,
                timeout=30,
            )
            self.assertEqual(proc.returncode, 0, proc.stderr)
            dashboard = json.loads((out / "dashboard.json").read_text())
            rows_by_id = {row["rom"]["id"]: row for row in dashboard["rows"]}
            # The sweep now mints a `pack` receipt bound to `discover` and,
            # only when pack passed, a `recompile` receipt bound to `pack`.
            # That satisfies the aggregator's contiguous-stage predecessor
            # rule, so highest-passed-stage advances past `discover`.
            self.assertEqual(rows_by_id["passed-rom"]["highest_passed_stage"], "recompile")
            self.assertEqual(rows_by_id["frontier-rom"]["status"], "blocked_at_recompile")
            self.assertEqual(rows_by_id["failed-rom"]["status"], "blocked_at_pack")
            self.assertEqual(rows_by_id["failed-rom"]["blocker_kind"], "NoUniqueAdmittedTable")


class ResumeTests(RunHelpers, unittest.TestCase):
    def test_resume_skips_roms_with_a_complete_attempt_for_same_candidate(self) -> None:
        # First pass: passed-rom (pack passes, recompile passes) and
        # failed-rom (pack is a frontier, no recompile receipt) -- both are
        # "complete": nothing further would change for the same candidate.
        first = self.run_sweep(["--rom-ids", self._rom_ids_file(["passed-rom", "failed-rom"])])
        self.assertEqual(first.returncode, 0, first.stderr)
        pack_before = {r["rom"]["id"]: r["receipt_id"] for r in self.receipts_for_stage("pack")}
        recompile_before = {r["receipt_id"] for r in self.receipts_for_stage("recompile")}
        self.assertEqual(set(pack_before), {"passed-rom", "failed-rom"})
        self.assertEqual(len(recompile_before), 1)

        second = self.run_sweep(["--resume"])
        self.assertEqual(second.returncode, 0, second.stderr)

        pack_after_by_rom: dict[str, list[str]] = {}
        for r in self.receipts_for_stage("pack"):
            pack_after_by_rom.setdefault(r["rom"]["id"], []).append(r["receipt_id"])
        recompile_after_by_rom: dict[str, list[str]] = {}
        for r in self.receipts_for_stage("recompile"):
            recompile_after_by_rom.setdefault(r["rom"]["id"], []).append(r["receipt_id"])

        # passed-rom and failed-rom must not have gained a second pack
        # receipt: --resume skipped them.
        self.assertEqual(len(pack_after_by_rom["passed-rom"]), 1)
        self.assertEqual(pack_after_by_rom["passed-rom"][0], pack_before["passed-rom"])
        self.assertEqual(len(pack_after_by_rom["failed-rom"]), 1)
        self.assertEqual(pack_after_by_rom["failed-rom"][0], pack_before["failed-rom"])
        self.assertEqual(set(recompile_after_by_rom.get("passed-rom", [])), recompile_before)
        self.assertNotIn("failed-rom", recompile_after_by_rom)

        # Every ROM now has exactly one pack receipt (the newly-run ones plus
        # the two resumed ones).
        for rom_id in ROM_IDS:
            self.assertEqual(len(pack_after_by_rom.get(rom_id, [])), 1, rom_id)
        # Only passed-rom and frontier-rom's packs passed, so only they hold
        # a recompile receipt.
        self.assertEqual(set(recompile_after_by_rom), {"passed-rom", "frontier-rom"})
        for rom_id, ids in recompile_after_by_rom.items():
            self.assertEqual(len(ids), 1, rom_id)

    def _rom_ids_file(self, ids: list[str]) -> str:
        path = self.root / "rom-ids.txt"
        path.write_text("\n".join(ids) + "\n")
        return str(path)


class PathFreedomTests(RunHelpers, unittest.TestCase):
    def test_no_absolute_path_in_stdout_or_receipts(self) -> None:
        proc = self.run_sweep()
        self.assertEqual(proc.returncode, 0, proc.stderr)
        rom_dir_text = str(self.rom_dir)
        self.assertNotIn(rom_dir_text, proc.stdout)
        for path in sorted((self.campaign_dir / "receipts").rglob("*.json")):
            text = path.read_text()
            self.assertNotIn(rom_dir_text, text)
            self.assertNotIn(str(self.root), text)


if __name__ == "__main__":
    unittest.main()
