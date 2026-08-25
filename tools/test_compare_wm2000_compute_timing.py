import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import compare_wm2000_compute_timing as timing


CHAIN_A = """\
[compute-gpu-timing] semantic_dispatches=4 passes=2 period_ns=1.0 span_ms=Some(1.500) valid_sum_ms=1.400 invalid_dispatches=0 dispatches(draws,pixels,ms)=[]
[compute-chain-timing] dispatches=4 draws=6 pixels=100 prepare_ms=1.000 resources_ms=0.100 uploads_ms=0.200 bind_groups_ms=0.300 encode_ms=0.400 submit_ms=0.500 wait_ms=2.000 gpu_map_ms=0.600 status_map_ms=0.700 target_map_ms=0.800 total_ms=6.600
[compute-gpu-timing] semantic_dispatches=2 passes=1 period_ns=1.0 span_ms=None valid_sum_ms=0.900 invalid_dispatches=1 dispatches(draws,pixels,ms)=[]
[compute-chain-timing] dispatches=2 draws=3 pixels=50 prepare_ms=0.500 resources_ms=0.050 uploads_ms=0.100 bind_groups_ms=0.150 encode_ms=0.200 submit_ms=0.250 wait_ms=1.000 gpu_map_ms=0.300 status_map_ms=0.350 target_map_ms=0.400 total_ms=3.300
[task-compute-census] tasks=10 members=40 compute_segments=2 compute_members=5 cpu_members=35 compute_total_ms=9.000 compute_ms/member=1.800 timed_cpu_members=35 timed_cpu_total_ms=7.000 timed_cpu_ms/member=0.200 registry_clone_calls=0 registry_clone_bytes=0 registry_clone_total_ms=0 shadow_clone_calls=0 shadow_clone_bytes=0 shadow_clone_total_ms=0
[task-batch-phase] tasks=10 members=40 total_ms=20.000 ms/task=2.000 ms/member=0.500
[task-batch-phase]   setup            1.000 ms    0.100 ms/task
[task-batch-phase]   session+other    3.000 ms    0.300 ms/task
"""


class CompareTimingTests(unittest.TestCase):
    def write_log(self, directory: Path, name: str, text: str) -> Path:
        path = directory / name
        path.write_text(text)
        return path

    def test_summary_keeps_totals_and_per_chain_checkpoint_denominators(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            path = self.write_log(Path(raw), "a.log", CHAIN_A)
            summary = timing.summarize(timing.parse_log(path))

        self.assertEqual(summary["chains"], 2)
        self.assertEqual(summary["chain_total.dispatches"], 6)
        self.assertAlmostEqual(summary["chain_total.total_ms"], 9.9)
        self.assertAlmostEqual(summary["per_chain.total_ms"], 4.95)
        self.assertEqual(summary["gpu_total.passes"], 3)
        self.assertEqual(summary["gpu_valid_spans"], 1)
        self.assertEqual(summary["gpu_total.span_ms"], 1.5)
        self.assertEqual(summary["checkpoints"], 5)
        self.assertAlmostEqual(summary["per_checkpoint.total_ms"], 1.98)
        self.assertAlmostEqual(summary["per_checkpoint.passes"], 0.6)
        self.assertEqual(summary["task_batch_phase.setup_ms"], 1)
        self.assertEqual(summary["task_batch_phase.session+other_ms"], 3)

    def test_comparison_reports_absolute_delta_and_percent(self) -> None:
        rows = timing.comparison({"x": 4.0, "zero": 0.0}, {"x": 3.0, "zero": 1.0})
        by_name = {row["metric"]: row for row in rows}
        self.assertEqual(by_name["x"]["delta"], -1)
        self.assertEqual(by_name["x"]["delta_percent"], -25)
        self.assertIsNone(by_name["zero"]["delta_percent"])

    def test_parser_names_truncated_chain_record(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            path = self.write_log(Path(raw), "bad.log", "[compute-chain-timing] total_ms=1\n")
            with self.assertRaisesRegex(ValueError, r"bad\.log:1:.*missing dispatches"):
                timing.parse_log(path)

    def test_json_cli_is_finite_and_machine_readable(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            directory = Path(raw)
            baseline = self.write_log(directory, "base.log", CHAIN_A)
            candidate = self.write_log(
                directory,
                "candidate.log",
                CHAIN_A.replace("total_ms=6.600", "total_ms=5.600", 1),
            )
            result = subprocess.run(
                [sys.executable, timing.__file__, str(baseline), str(candidate), "--json"],
                check=True,
                capture_output=True,
                text=True,
            )
            report = json.loads(result.stdout)

        self.assertEqual(report["schema"], "fn64.wm2000-compute-timing-comparison.v1")
        total = next(row for row in report["metrics"] if row["metric"] == "chain_total.total_ms")
        self.assertAlmostEqual(total["delta"], -1)


if __name__ == "__main__":
    unittest.main()
