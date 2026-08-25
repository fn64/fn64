#!/usr/bin/env python3
"""Correlate per-task renderer timing with WM2000 drawn-frame latency."""

from __future__ import annotations

import argparse
import json
import pathlib
import re
from collections import defaultdict
from dataclasses import dataclass

from summarize_wm2000_pump_census import BUDGET_MS, parse_pumps, percentile


TASK_RE = re.compile(
    r"^\[task-compute-tail\] task=(\d+) members=(\d+) cpu_members=(\d+) "
    r"compute_members=(\d+) compute_ms=([0-9.]+) "
    r"(?:programs=(.*?) )?cpu=(.*)$"
)


@dataclass(frozen=True)
class Task:
    ordinal: int
    members: int
    cpu_members: int
    compute_members: int
    compute_ms: float
    programs: dict[str, tuple[int, int, float]]
    cpu: dict[str, tuple[int, float]]


def parse_tasks(text: str) -> list[Task]:
    tasks = []
    for line in text.splitlines():
        match = TASK_RE.match(line)
        if match is None:
            continue
        programs: dict[str, tuple[int, int, float]] = {}
        fields = match.group(6).split(";") if match.group(6) else []
        for field in fields:
            try:
                program, value = field.rsplit("=", 1)
                segments, members, elapsed_ms = value.split(":", 2)
                programs[program] = (int(segments), int(members), float(elapsed_ms))
            except ValueError as error:
                raise ValueError(f"malformed task compute program: {field}") from error
        reasons: dict[str, tuple[int, float]] = {}
        fields = match.group(7).split(";") if match.group(7) else []
        for field in fields:
            try:
                reason, value = field.rsplit("=", 1)
                members, elapsed_ms = value.split(":", 1)
                reasons[reason] = (int(members), float(elapsed_ms))
            except ValueError as error:
                raise ValueError(f"malformed task CPU reason: {field}") from error
        tasks.append(
            Task(
                ordinal=int(match.group(1)),
                members=int(match.group(2)),
                cpu_members=int(match.group(3)),
                compute_members=int(match.group(4)),
                compute_ms=float(match.group(5)),
                programs=programs,
                cpu=reasons,
            )
        )
    if not tasks:
        raise ValueError(
            "no [task-compute-tail] rows found; set FN64_TASK_COMPUTE_TAIL_CENSUS=1"
        )
    return tasks


def mean(values: list[float]) -> float:
    return sum(values) / len(values) if values else 0.0


def population(frames: list[dict[str, object]]) -> dict[str, object]:
    reasons: dict[str, list[float | int]] = defaultdict(lambda: [0, 0.0])
    programs: dict[str, list[float | int]] = defaultdict(lambda: [0, 0, 0.0])
    for frame in frames:
        for reason, value in frame["cpu_reasons"].items():
            reasons[reason][0] += value["members"]
            reasons[reason][1] += value["elapsed_ms"]
        for program, value in frame["compute_programs"].items():
            programs[program][0] += value["segments"]
            programs[program][1] += value["members"]
            programs[program][2] += value["elapsed_ms"]
    count = len(frames)
    return {
        "count": count,
        "mean_drawn_frame_ms": mean([float(frame["drawn_frame_ms"]) for frame in frames]),
        "mean_rdp_ms": mean([float(frame["rdp_ms"]) for frame in frames]),
        "mean_task_cpu_ms": mean([float(frame["task_cpu_ms"]) for frame in frames]),
        "mean_task_compute_ms": mean([float(frame["task_compute_ms"]) for frame in frames]),
        "compute_programs": {
            program: {
                "segments_per_frame": values[0] / count if count else 0.0,
                "members_per_frame": values[1] / count if count else 0.0,
                "ms_per_frame": values[2] / count if count else 0.0,
            }
            for program, values in sorted(
                programs.items(), key=lambda item: item[1][2], reverse=True
            )
        },
        "cpu_reasons": {
            reason: {
                "members_per_frame": values[0] / count if count else 0.0,
                "ms_per_frame": values[1] / count if count else 0.0,
            }
            for reason, values in sorted(
                reasons.items(), key=lambda item: item[1][1], reverse=True
            )
        },
    }


def summarize(text: str) -> dict[str, object]:
    pumps = parse_pumps(text)
    all_tasks = parse_tasks(text)
    measured_task_count = sum(pump.gfx_tasks for pump in pumps)
    if len(all_tasks) < measured_task_count:
        raise ValueError(
            f"only {len(all_tasks)} task rows cover {measured_task_count} measured graphics tasks"
        )

    # Task timing starts at process boot, while the pump sequence starts after
    # warmup. The benchmark exits as soon as the measured sequence is complete,
    # so its exact task population is the suffix after the warmup-only prefix.
    tasks = all_tasks[-measured_task_count:] if measured_task_count else []
    task_cursor = 0
    pump_tasks: list[list[Task]] = []
    for pump in pumps:
        next_cursor = task_cursor + pump.gfx_tasks
        pump_tasks.append(tasks[task_cursor:next_cursor])
        task_cursor = next_cursor
    if task_cursor != len(tasks):
        raise ValueError("measured graphics-task population did not close")

    swap_indices = [pump.index for pump in pumps if pump.swapped]
    frames = []
    for previous, current in zip(swap_indices, swap_indices[1:]):
        span = pumps[previous + 1 : current + 1]
        frame_tasks = [
            task
            for index in range(previous + 1, current + 1)
            for task in pump_tasks[index]
        ]
        reasons: dict[str, list[float | int]] = defaultdict(lambda: [0, 0.0])
        programs: dict[str, list[float | int]] = defaultdict(lambda: [0, 0, 0.0])
        for task in frame_tasks:
            for reason, (members, elapsed_ms) in task.cpu.items():
                reasons[reason][0] += members
                reasons[reason][1] += elapsed_ms
            for program, (segments, members, elapsed_ms) in task.programs.items():
                programs[program][0] += segments
                programs[program][1] += members
                programs[program][2] += elapsed_ms
        frames.append(
            {
                "pump": current,
                "drawn_frame_ms": sum(pump.wall_ms for pump in span),
                "rdp_ms": sum(pump.gfx_lle_rdp_ms for pump in span),
                "task_count": len(frame_tasks),
                "task_cpu_ms": sum(
                    elapsed_ms
                    for task in frame_tasks
                    for _, elapsed_ms in task.cpu.values()
                ),
                "task_compute_ms": sum(task.compute_ms for task in frame_tasks),
                "compute_programs": {
                    program: {
                        "segments": values[0],
                        "members": values[1],
                        "elapsed_ms": values[2],
                    }
                    for program, values in programs.items()
                },
                "cpu_reasons": {
                    reason: {"members": values[0], "elapsed_ms": values[1]}
                    for reason, values in reasons.items()
                },
            }
        )
    if not frames:
        raise ValueError("at least two post-warmup VI swaps are required")

    ordered = sorted(frames, key=lambda frame: float(frame["drawn_frame_ms"]), reverse=True)
    top_count = max(1, (len(frames) + 19) // 20)
    drawn_ms = [float(frame["drawn_frame_ms"]) for frame in frames]
    return {
        "schema": "fn64.wm2000-task-tail.v1",
        "pumps": len(pumps),
        "drawn_frames": len(frames),
        "warmup_task_rows": len(all_tasks) - measured_task_count,
        "measured_task_rows": measured_task_count,
        "drawn_frame_ms": {
            "p95": percentile(drawn_ms, 0.95),
            "max": max(drawn_ms),
        },
        "within_budget": population(
            [frame for frame in frames if float(frame["drawn_frame_ms"]) <= BUDGET_MS]
        ),
        "over_budget": population(
            [frame for frame in frames if float(frame["drawn_frame_ms"]) > BUDGET_MS]
        ),
        "slowest_five_percent": population(ordered[:top_count]),
        "slowest_frames": ordered[:20],
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("log", type=pathlib.Path)
    parser.add_argument("--output", type=pathlib.Path)
    args = parser.parse_args()
    try:
        result = summarize(args.log.read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        parser.error(str(error))
    encoded = json.dumps(result, indent=2, sort_keys=True) + "\n"
    if args.output is not None:
        args.output.write_text(encoded, encoding="utf-8")
    else:
        print(encoded, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
