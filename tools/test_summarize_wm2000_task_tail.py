#!/usr/bin/env python3

import importlib.util
import pathlib
import sys
import unittest


TOOLS = pathlib.Path(__file__).parent
for name in ("summarize_wm2000_pump_census", "summarize_wm2000_task_tail"):
    spec = importlib.util.spec_from_file_location(name, TOOLS / f"{name}.py")
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)

from summarize_wm2000_task_tail import parse_tasks, summarize


def pump(index: int, swapped: bool, gfx_tasks: int, wall_ms: float) -> str:
    return (
        f"[pump-seq] {index},{wall_ms:.4f},1,{int(swapped)},{gfx_tasks},0,0,0,0,"
        f"{max(0.0, wall_ms - 1.0):.4f},0,0,0,0,0"
    )


def task(ordinal: int, cpu: str, compute_ms: float = 0.0) -> str:
    return (
        f"[task-compute-tail] task={ordinal} members=2 cpu_members=1 "
        f"compute_members=1 compute_ms={compute_ms:.3f} cpu={cpu}"
    )


class TaskTailTest(unittest.TestCase):
    def test_reason_delimiter_does_not_conflict_with_debug_punctuation(self) -> None:
        parsed = parse_tasks(
            task(
                1,
                "ExactAdmissionRejected(ProgramBits([1, 2, 3, 4]))=2:1.250;"
                "Planned(NoRawTriangle)=1:0.500",
            )
        )
        self.assertEqual(len(parsed), 1)
        self.assertEqual(parsed[0].cpu["Planned(NoRawTriangle)"], (1, 0.5))

    def test_warmup_prefix_is_removed_and_frame_populations_close(self) -> None:
        text = "\n".join(
            [
                "[pump-census] RENDERER: wgpu",
                task(1, "Planned(NoRawTriangle)=1:99.000"),
                task(2, "program_bits:1/2/3/4=1:2.000", 1.0),
                task(3, "cycle_type:5/6/7/8=1:20.000"),
                pump(0, True, 0, 1.0),
                pump(1, False, 1, 10.0),
                pump(2, True, 0, 10.0),
                pump(3, False, 1, 20.0),
                pump(4, True, 0, 20.0),
            ]
        )
        result = summarize(text)
        self.assertEqual(result["warmup_task_rows"], 1)
        self.assertEqual(result["measured_task_rows"], 2)
        self.assertEqual(result["within_budget"]["count"], 1)
        self.assertEqual(result["over_budget"]["count"], 1)
        self.assertEqual(result["over_budget"]["mean_task_cpu_ms"], 20.0)
        self.assertNotIn(
            "Planned(NoRawTriangle)", result["over_budget"]["cpu_reasons"]
        )


if __name__ == "__main__":
    unittest.main()
