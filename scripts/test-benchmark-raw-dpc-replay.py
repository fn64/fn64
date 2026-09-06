#!/usr/bin/env python3
"""ROM-free tests for benchmark-raw-dpc-replay.py."""

from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "benchmark-raw-dpc-replay.py"
FAKE = """#!/usr/bin/env python3
import os
identity = os.environ.get('FAKE_ID', 'stable')
value = float(os.environ.get('FAKE_MS', '10'))
print('selected_window_packets=140 replay_packets=140 combined_window=1 task_batch_window=1 task_batches=3 terminal_stream_bytes=8 warmup=10 repeat=1 committed_fnv1a=' + identity + ' postimage_sha256=post-' + identity)
for name, scale in [('execute', 1.0), ('total', 2.0)]:
    sample = value * scale
    print(f'{name:>11} mean_ms={sample:.3f} p50_ms={sample:.3f} p95_ms={sample:.3f} p99_ms={sample:.3f} max_ms={sample:.3f}')
"""


class ReplayBenchmarkTest(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory(prefix="fn64-replay-loop-test.")
        self.root = Path(self.temp.name)
        self.binary = self.root / "fake-replay"
        self.binary.write_text(FAKE)
        self.binary.chmod(0o700)
        self.tools = self.root / "tools"
        self.tools.mkdir()
        fake_ps = self.tools / "ps"
        fake_ps.write_text("#!/bin/sh\nexit 0\n")
        fake_ps.chmod(0o700)
        self.streams = self.root / "streams"
        self.streams.mkdir()
        (self.streams / "raw-dpc-000001-xbus.bin").write_bytes(b"12345678")
        self.rdram = self.root / "rdram.bin"
        self.rdram.write_bytes(b"private fixture")

    def tearDown(self) -> None:
        self.temp.cleanup()

    def run_tool(self, mode: str, candidate_ms: str = "8", extra: list[str] | None = None):
        output = self.root / ("output-" + mode + "-" + candidate_ms)
        command = [
            sys.executable, str(SCRIPT),
            "--control-bin", str(self.binary), "--candidate-bin", str(self.binary),
            "--streams", str(self.streams), "--rdram", str(self.rdram),
            "--output-dir", str(output), "--mode", mode,
            "--packet", "1", "--window", "1", "--task-batch",
            "--control-env", "FAKE_MS=10", "--candidate-env", f"FAKE_MS={candidate_ms}",
            "--max-load-one", "100000",
        ]
        env = dict(os.environ)
        env["PATH"] = str(self.tools) + os.pathsep + env["PATH"]
        process = subprocess.run(
            command + (extra or []), capture_output=True, text=True, env=env
        )
        summary = json.loads((output / "summary.json").read_text()) if (output / "summary.json").exists() else None
        return process, summary

    def test_scout_brackets_candidate_then_completes_two_plus_two(self) -> None:
        process, summary = self.run_tool("scout")
        self.assertEqual(process.returncode, 0, process.stderr)
        self.assertEqual(summary["status"], "scout_complete")
        self.assertEqual([leg["lane"] for leg in summary["legs"]],
                         ["control", "candidate", "control", "candidate"])
        self.assertEqual(summary["identity"]["committed_fnv1a"], "stable")
        encoded = json.dumps(summary)
        self.assertNotIn(str(self.root), encoded)
        self.assertNotIn("FAKE_MS", encoded)

    def test_promotion_runs_six_plus_six_timing_and_four_identity_closures(self) -> None:
        process, summary = self.run_tool("promote")
        self.assertEqual(process.returncode, 0, process.stderr)
        self.assertEqual(summary["status"], "promoted_complete")
        self.assertEqual(len(summary["legs"]), 16)
        self.assertEqual(sum(leg["lane"] == "control" for leg in summary["legs"]), 6)
        self.assertEqual(sum(leg["lane"] == "candidate" for leg in summary["legs"]), 10)
        self.assertEqual(sum(leg["phase"] == "identity" for leg in summary["legs"]), 4)
        self.assertEqual(summary["configuration"]["timing_control_runs"], 6)
        self.assertEqual(summary["configuration"]["timing_candidate_runs"], 6)
        self.assertEqual(summary["configuration"]["identity_candidate_runs"], 4)
        self.assertEqual(len(summary["comparison"]["pair_candidate_minus_control_ms"]), 6)

    def test_obvious_regression_stops_after_three_bracketed_processes(self) -> None:
        process, summary = self.run_tool("promote", candidate_ms="12")
        self.assertEqual(process.returncode, 0, process.stderr)
        self.assertEqual(summary["status"], "obvious_regression")
        self.assertEqual(len(summary["legs"]), 3)
        self.assertEqual(
            [leg["lane"] for leg in summary["legs"]],
            ["control", "candidate", "control"],
        )
        self.assertEqual(summary["regression_decision"]["guardrail_ms"], 1.0)

    def test_sub_guardrail_result_does_not_stop_early(self) -> None:
        process, summary = self.run_tool("scout", candidate_ms="10.5")
        self.assertEqual(process.returncode, 0, process.stderr)
        self.assertEqual(summary["status"], "scout_complete")
        self.assertEqual(len(summary["legs"]), 4)

    def test_identity_mismatch_is_a_hard_failure(self) -> None:
        process, summary = self.run_tool(
            "scout", extra=["--candidate-env", "FAKE_ID=different"]
        )
        self.assertNotEqual(process.returncode, 0)
        self.assertEqual(summary["status"], "identity_mismatch")
        self.assertNotIn(str(self.root), json.dumps(summary))
        self.assertIn("identity mismatch", process.stderr)

    def test_quiet_machine_preflight_rejects_compiler_process(self) -> None:
        fake_ps = self.tools / "ps"
        fake_ps.write_text("#!/bin/sh\nprintf '%s\\n' /usr/bin/cargo\n")
        process, summary = self.run_tool("scout")
        self.assertNotEqual(process.returncode, 0)
        self.assertIsNone(summary)
        self.assertIn("quiet-machine preflight", process.stderr)


if __name__ == "__main__":
    unittest.main()
