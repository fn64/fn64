#!/usr/bin/env python3
"""Focused synthetic tests for the ten-run snapshot loader A/B series gate."""

from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import tempfile
import textwrap
import unittest


ROOT = Path(__file__).resolve().parents[2]
SERIES = ROOT / "tools/ghidra/run-snapshot-loader-ab-series.py"


def executable(path: Path, body: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(textwrap.dedent(body).lstrip(), encoding="utf-8")
    path.chmod(path.stat().st_mode | stat.S_IXUSR)


class SnapshotLoaderAbSeriesTest(unittest.TestCase):
    def fixture(self, root: Path) -> tuple[Path, dict[str, str], list[str]]:
        repo = root / "repo"
        tools = repo / "tools/ghidra"
        tools.mkdir(parents=True)
        shutil.copy2(SERIES, tools / SERIES.name)
        (tools / SERIES.name).chmod(0o700)
        executable(
            tools / "run-snapshot-loader-ab.sh",
            r'''
            #!/usr/bin/env python3
            import hashlib, json, os, pathlib, sys
            _, snapshot, bank, materialized, workspace, extension, conformance = sys.argv
            workspace = pathlib.Path(workspace)
            index = len(list(workspace.glob("ghidra-snapshot-loader-ab.*"))) + 1
            attempt = workspace / f"ghidra-snapshot-loader-ab.{index:03d}"
            (attempt / "out").mkdir(parents=True)
            comparison = attempt / "out/comparison.json"
            comparison.write_text(json.dumps({"stable": os.environ.get("FN64_SYNTH_DRIFT_RUN") != str(index)}) + "\n")
            comparison_sha = hashlib.sha256(comparison.read_bytes()).hexdigest()
            sha = "a" * 64
            artifacts = {name: sha for name in (
                "evidence", "binary_config", "n64_config", "binary_pre", "binary_post",
                "n64_pre", "n64_post", "n64loaderwv_install_verification",
                "n64loaderwv_runtime_verification", "comparison",
            )}
            artifacts["comparison"] = comparison_sha
            receipt = {
                "schema": "fn64.ghidra-snapshot-loader-ab-receipt", "schema_version": 1,
                "candidate_only": True, "role": "differential_comparison",
                "context": "synthetic_zero_fill", "program_snapshot_sha256": sha,
                "input": {"bank": bank}, "n64loaderwv": {"repository": "fn64/N64LoaderWV"},
                "tool_identity_sha256": {"runner": sha}, "artifact_sha256": artifacts,
                "resource_evidence_sha256": {
                    "distribution_scan": f"{index:064x}", "binary": sha, "n64": sha,
                    "comparison": sha, "distribution_verify": sha,
                },
                "completed_lanes": ["binary-loader", "n64loaderwv"],
                "production_ingest_performed": False,
            }
            receipt_path = attempt / "out/receipt.json"
            receipt_path.write_text(json.dumps(receipt, sort_keys=True, separators=(",", ":")) + "\n")
            if os.environ.get("FN64_SYNTH_FAIL_RUN") == str(index):
                raise SystemExit(1)
            print("ghidra snapshot-loader-ab: complete")
            print(f"attempt={attempt}")
            print(f"comparison={comparison}")
            print(f"receipt={receipt_path}")
            ''',
        )
        inputs = []
        for name in ("snapshot.json", "bank.bin", "extension.zip", "conformance.txt"):
            path = root / name
            path.write_bytes(name.encode())
            inputs.append(str(path))
        environment = dict(os.environ)
        return tools / SERIES.name, environment, inputs

    def invoke(self, runner: Path, environment: dict[str, str], inputs: list[str], series: Path) -> subprocess.CompletedProcess[str]:
        series.mkdir(mode=0o700)
        environment = dict(environment, FN64_GHIDRA_LOADER_AB_SERIES_WORK=str(series))
        return subprocess.run(
            [str(runner), inputs[0], "bank-a", inputs[1], inputs[2], inputs[3]],
            env=environment,
            text=True,
            capture_output=True,
            check=False,
        )

    def test_ten_runs_allow_only_resource_evidence_drift(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            runner, environment, inputs = self.fixture(root)
            series = root / "series"
            completed = self.invoke(runner, environment, inputs, series)
            self.assertEqual(completed.returncode, 0, completed.stderr)
            receipt = json.loads((series / "receipt.json").read_text())
            self.assertEqual(receipt["run_count"], 10)
            self.assertEqual(receipt["required_clean_runs"], 10)
            self.assertFalse(receipt["production_ingest_performed"])
            records = [json.loads(line) for line in (series / "attempt-receipts.jsonl").read_text().splitlines()]
            self.assertEqual([record["run"] for record in records], list(range(1, 11)))
            self.assertEqual(len({record["resource_evidence_sha256"]["distribution_scan"] for record in records}), 10)
            for published in (series / "receipt.json", series / "semantic-receipt.json", series / "attempt-receipts.jsonl"):
                self.assertNotIn(str(root), published.read_text())

    def test_semantic_artifact_drift_fails_fast_without_series_receipt(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            runner, environment, inputs = self.fixture(root)
            environment["FN64_SYNTH_DRIFT_RUN"] = "3"
            series = root / "series"
            completed = self.invoke(runner, environment, inputs, series)
            self.assertEqual(completed.returncode, 2)
            self.assertIn("semantic receipt or artifact identity drifted", completed.stderr)
            self.assertFalse((series / "receipt.json").exists())
            self.assertFalse((series / "semantic-receipt.json").exists())
            self.assertFalse((series / "attempt-receipts.jsonl").exists())
            self.assertFalse((series / "workspace/ghidra-snapshot-loader-ab.004").exists())

    def test_attempt_failure_fails_fast_without_series_receipt(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            runner, environment, inputs = self.fixture(root)
            environment["FN64_SYNTH_FAIL_RUN"] = "2"
            series = root / "series"
            completed = self.invoke(runner, environment, inputs, series)
            self.assertEqual(completed.returncode, 2)
            self.assertIn("run 2/10 failed", completed.stderr)
            self.assertFalse((series / "receipt.json").exists())
            self.assertFalse((series / "workspace/ghidra-snapshot-loader-ab.003").exists())


if __name__ == "__main__":
    unittest.main()
