#!/usr/bin/env python3
"""Tests for scripts/corpus-campaign.zsh.

No ROMs, no cargo build: static checks on the script text, a --dry-run
execution that prints its plan without running anything, and an isolated
exec of the timing-summary python heredoc against hand-written receipts.
"""

from __future__ import annotations

import json
import os
import re
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT_PATH = REPO_ROOT / "scripts" / "corpus-campaign.zsh"


def read_script_text() -> str:
    return SCRIPT_PATH.read_text()


class StaticChecksTest(unittest.TestCase):
    def test_script_exists(self) -> None:
        self.assertTrue(SCRIPT_PATH.is_file(), f"{SCRIPT_PATH} does not exist")

    def test_script_is_executable(self) -> None:
        mode = SCRIPT_PATH.stat().st_mode
        self.assertTrue(mode & stat.S_IXUSR, f"{SCRIPT_PATH} is not executable")

    def test_has_set_dash_e_u_o_pipefail(self) -> None:
        text = read_script_text()
        has_combined = re.search(r"^set\s+-euo\s+pipefail\s*$", text, re.MULTILINE)
        has_split = (
            re.search(r"^set\s+-e[ug]*\b", text, re.MULTILINE)
            and re.search(r"^set\s+-u[ge]*\b", text, re.MULTILINE)
            and re.search(r"^set\s+-o\s+pipefail\s*$", text, re.MULTILINE)
        )
        self.assertTrue(
            has_combined or has_split,
            "script must set -e, -u, and pipefail (combined or split form)",
        )

    def test_never_references_tmp(self) -> None:
        text = read_script_text()
        # Allow the word "tmp" only as part of write_new-style unique temp
        # filenames the scripts themselves create *inside* the campaign dir
        # (this script does not create any), but never a literal /tmp path.
        self.assertNotIn("/tmp", text, "script must never reference /tmp")
        self.assertNotIn("/private/tmp", text, "script must never reference /private/tmp")

    def test_refuses_without_rom_dir(self) -> None:
        result = subprocess.run(
            ["zsh", str(SCRIPT_PATH)],
            cwd=REPO_ROOT,
            capture_output=True,
            text=True,
            timeout=30,
        )
        self.assertNotEqual(result.returncode, 0, "must exit nonzero with no args")
        combined = result.stdout + result.stderr
        self.assertIn("usage:", combined.lower())

    def test_help_exits_zero(self) -> None:
        result = subprocess.run(
            ["zsh", str(SCRIPT_PATH), "--help"],
            cwd=REPO_ROOT,
            capture_output=True,
            text=True,
            timeout=30,
        )
        self.assertEqual(result.returncode, 0)


class DryRunTest(unittest.TestCase):
    def run_dry(self, extra_args: list[str], rom_dir: Path) -> subprocess.CompletedProcess:
        return subprocess.run(
            ["zsh", str(SCRIPT_PATH), "--rom-dir", str(rom_dir), "--dry-run", *extra_args],
            cwd=REPO_ROOT,
            capture_output=True,
            text=True,
            timeout=30,
        )

    def test_dry_run_prints_five_stages_in_order_and_exits_zero(self) -> None:
        with tempfile.TemporaryDirectory() as rom_dir:
            result = self.run_dry([], Path(rom_dir))
        self.assertEqual(result.returncode, 0, result.stderr)
        stdout = result.stdout

        stage_markers = [
            "stage1 catalog:",
            "stage1 rom-map:",
            "stage2 rom-frontier:",
            "stage3 import:",
            "stage4 recompile-sweep:",
            "stage5 dashboard:",
            "stage5 rank:",
        ]
        positions = []
        for marker in stage_markers:
            self.assertIn(marker, stdout, f"missing stage marker: {marker}")
            positions.append(stdout.index(marker))
        self.assertEqual(
            positions,
            sorted(positions),
            "stage markers must appear in order",
        )

        # Five distinct pipeline stages named in the spec: rom-catalog,
        # rom-frontier, import-rom-frontier-campaign, corpus-recompile-sweep,
        # corpus-dashboard (+ corpus-unblock-rank folded into stage5).
        for script_name in (
            "rom-catalog.py",
            "rom-frontier.py",
            "import-rom-frontier-campaign.py",
            "corpus-recompile-sweep.py",
            "corpus-dashboard.py",
            "corpus-unblock-rank.py",
        ):
            self.assertIn(script_name, stdout, f"plan must mention {script_name}")

        # Never runs anything for real.
        self.assertNotIn("corpus-campaign: building fn64-discover", stdout)
        self.assertNotIn("corpus-campaign: timing summary", stdout)

    def test_dry_run_honors_limit(self) -> None:
        with tempfile.TemporaryDirectory() as rom_dir:
            result = self.run_dry(["--limit", "3"], Path(rom_dir))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("--limit 3", result.stdout)
        self.assertIn("limit: 3", result.stdout)

    def test_dry_run_honors_resume(self) -> None:
        with tempfile.TemporaryDirectory() as rom_dir, tempfile.TemporaryDirectory() as resume_dir:
            result = self.run_dry(["--resume", resume_dir], Path(rom_dir))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("resume: yes", result.stdout)
        self.assertIn(resume_dir, result.stdout)
        self.assertIn("--resume", result.stdout)

    def test_dry_run_without_resume_reports_no(self) -> None:
        with tempfile.TemporaryDirectory() as rom_dir:
            result = self.run_dry([], Path(rom_dir))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("resume: no", result.stdout)

    def test_dry_run_honors_skip_build(self) -> None:
        with tempfile.TemporaryDirectory() as rom_dir:
            result = self.run_dry(["--skip-build"], Path(rom_dir))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("skip-build: 1", result.stdout)
        self.assertIn("1) build: skipped (--skip-build)", result.stdout)

    def test_dry_run_without_skip_build_plans_a_build(self) -> None:
        with tempfile.TemporaryDirectory() as rom_dir:
            result = self.run_dry([], Path(rom_dir))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("1) build: CARGO_INCREMENTAL=0 cargo build", result.stdout)

    def test_dry_run_rejects_missing_rom_dir_argument(self) -> None:
        result = subprocess.run(
            ["zsh", str(SCRIPT_PATH), "--dry-run"],
            cwd=REPO_ROOT,
            capture_output=True,
            text=True,
            timeout=30,
        )
        self.assertNotEqual(result.returncode, 0)


def extract_timing_heredoc(script_text: str) -> str:
    """Pull out the inline `python3 - "$campaign_dir/receipts" <<'PY' ... PY`
    heredoc that computes the p50/p95/max-RSS timing summary."""
    marker = 'python3 - "$campaign_dir/receipts" <<\'PY\''
    start = script_text.index(marker)
    body_start = script_text.index("\n", start) + 1
    end = script_text.index("\nPY", body_start)
    return script_text[body_start:end]


class TimingHeredocTest(unittest.TestCase):
    def setUp(self) -> None:
        self.heredoc_source = extract_timing_heredoc(read_script_text())

    def write_receipt(self, receipts_dir: Path, name: str, stage: str, wall_ms, rss_bytes) -> None:
        policy = {}
        if wall_ms is not None:
            policy["wall_time_ms"] = wall_ms
        if rss_bytes is not None:
            policy["peak_rss_bytes"] = rss_bytes
        receipt = {"stage": stage, "policy": policy}
        (receipts_dir / f"{name}.json").write_text(json.dumps(receipt))

    def run_heredoc(self, receipts_dir: Path) -> str:
        # Exec the extracted heredoc text exactly as the script would run it,
        # against a temp receipts dir, and capture its stdout.
        wrapper_script = (
            "import sys\n"
            f"sys.argv = ['-', {json.dumps(str(receipts_dir))}]\n"
            + self.heredoc_source
        )
        result = subprocess.run(
            [sys.executable, "-c", wrapper_script],
            capture_output=True,
            text=True,
            timeout=30,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        return result.stdout

    def test_computes_p50_p95_and_max_rss(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            receipts_dir = Path(tmp)
            # recompile: 5 receipts with distinct wall times/RSS.
            values = [(1000, 100), (2000, 200), (3000, 300), (4000, 400), (5000, 500)]
            for index, (wall, rss) in enumerate(values):
                self.write_receipt(receipts_dir, f"recompile-{index}", "recompile", wall, rss)
            output = self.run_heredoc(receipts_dir)

        line = next(line for line in output.splitlines() if line.strip().startswith("stage=recompile"))
        self.assertIn("receipts=5", line)
        self.assertIn("p50_wall_ms=3000", line)
        self.assertIn("max_peak_rss_bytes=500", line)

    def test_handles_empty_stage(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            receipts_dir = Path(tmp)
            self.write_receipt(receipts_dir, "recompile-0", "recompile", 1000, 100)
            # No pack receipts at all.
            output = self.run_heredoc(receipts_dir)

        pack_line = next(line for line in output.splitlines() if line.strip().startswith("stage=pack"))
        self.assertIn("receipts=0", pack_line)
        self.assertIn("p50_wall_ms=None", pack_line)
        self.assertIn("max_peak_rss_bytes=None", pack_line)

    def test_ignores_receipts_missing_policy_fields(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            receipts_dir = Path(tmp)
            self.write_receipt(receipts_dir, "pack-0", "pack", 1000, 100)
            self.write_receipt(receipts_dir, "pack-1", "pack", None, None)
            (receipts_dir / "pack-2.json").write_text(json.dumps({"stage": "pack"}))
            output = self.run_heredoc(receipts_dir)

        pack_line = next(line for line in output.splitlines() if line.strip().startswith("stage=pack"))
        self.assertIn("receipts=1", pack_line)

    def test_ignores_other_stage_receipts(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            receipts_dir = Path(tmp)
            self.write_receipt(receipts_dir, "discover-0", "discover", 999, 999)
            self.write_receipt(receipts_dir, "pack-0", "pack", 500, 50)
            output = self.run_heredoc(receipts_dir)

        pack_line = next(line for line in output.splitlines() if line.strip().startswith("stage=pack"))
        self.assertIn("receipts=1", pack_line)
        self.assertIn("p50_wall_ms=500", pack_line)

    def test_tolerates_malformed_json(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            receipts_dir = Path(tmp)
            self.write_receipt(receipts_dir, "recompile-0", "recompile", 1000, 100)
            (receipts_dir / "broken.json").write_text("{not json")
            output = self.run_heredoc(receipts_dir)

        line = next(line for line in output.splitlines() if line.strip().startswith("stage=recompile"))
        self.assertIn("receipts=1", line)

    def test_nested_receipt_subdirectories_are_scanned(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            receipts_dir = Path(tmp)
            nested = receipts_dir / "some-rom-id"
            nested.mkdir()
            self.write_receipt(nested, "recompile-0", "recompile", 1000, 100)
            output = self.run_heredoc(receipts_dir)

        line = next(line for line in output.splitlines() if line.strip().startswith("stage=recompile"))
        self.assertIn("receipts=1", line)


if __name__ == "__main__":
    unittest.main()
