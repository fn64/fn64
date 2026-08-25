#!/usr/bin/env python3
"""Summarize WM2000 swap-to-swap latency from a pump-census sequence dump."""

from __future__ import annotations

import argparse
import json
import math
import pathlib
import re
import sys
from collections import Counter
from dataclasses import dataclass


SEQUENCE_PREFIX = "[pump-seq] "
RENDERER_RE = re.compile(r"^\[pump-census\] RENDERER: (\S+)$", re.MULTILINE)
BUDGET_MS = 1000.0 / 30.0


@dataclass(frozen=True)
class Pump:
    index: int
    wall_ms: float
    swapped: bool
    steps: int
    gfx_tasks: int
    audio_tasks: int
    executor_ms: float
    gfx_ms: float
    gfx_lle_rsp_ms: float
    gfx_lle_rdp_ms: float
    audio_lle_ms: float
    vi_present_ms: float
    resume_dispatch_ms: float
    rsp_steps_gfx: int
    rsp_steps_audio: int
    task_completion_before: int | None = None
    task_completion_after: int | None = None
    task_envelope_ms: float | None = None
    task_hot_member_ms: float | None = None
    task_all_cpu_member_ms: float | None = None
    task_compute_segment_ms: float | None = None
    task_renderer_work_ms: float | None = None
    task_member_accounted_ms: float | None = None
    task_view_plan_residual_ms: float | None = None
    task_finalize_coordinator_ms: float | None = None
    task_post_view_wrapper_residual_ms: float | None = None
    task_outer_residual_ms: float | None = None
    task_rdp_outside_envelope_ms: float | None = None


METRICS = (
    "steps",
    "gfx_tasks",
    "audio_tasks",
    "executor_ms",
    "gfx_ms",
    "gfx_lle_rsp_ms",
    "gfx_lle_rdp_ms",
    "audio_lle_ms",
    "vi_present_ms",
    "resume_dispatch_ms",
    "rsp_steps_gfx",
    "rsp_steps_audio",
)

TASK_PHASE_METRICS = (
    "task_envelope_ms",
    "task_hot_member_ms",
    "task_all_cpu_member_ms",
    "task_compute_segment_ms",
    "task_renderer_work_ms",
    "task_member_accounted_ms",
    "task_view_plan_residual_ms",
    "task_finalize_coordinator_ms",
    "task_post_view_wrapper_residual_ms",
    "task_outer_residual_ms",
    "task_rdp_outside_envelope_ms",
)


def percentile(values: list[float], fraction: float) -> float:
    if not values:
        raise ValueError("a percentile requires at least one value")
    ordered = sorted(values)
    rank = max(1, math.ceil(fraction * len(ordered)))
    return ordered[rank - 1]


def parse_pumps(text: str) -> list[Pump]:
    pumps: list[Pump] = []
    for line in text.splitlines():
        if not line.startswith(SEQUENCE_PREFIX):
            continue
        fields = line[len(SEQUENCE_PREFIX) :].split(",")
        if len(fields) not in (15, 28):
            raise ValueError(
                f"pump sequence row has {len(fields)} fields, expected legacy 15 or expanded 28"
            )
        task_fields: dict[str, int | float | None] = {}
        if len(fields) == 28:
            task_fields = {
                "task_completion_before": int(fields[15]),
                "task_completion_after": int(fields[16]),
                **{
                    metric: float(value)
                    for metric, value in zip(TASK_PHASE_METRICS, fields[17:])
                },
            }
        pump = Pump(
            index=int(fields[0]),
            wall_ms=float(fields[1]),
            swapped=fields[3] == "1",
            steps=int(fields[2]),
            gfx_tasks=int(fields[4]),
            audio_tasks=int(fields[5]),
            executor_ms=float(fields[6]),
            gfx_ms=float(fields[7]),
            gfx_lle_rsp_ms=float(fields[8]),
            gfx_lle_rdp_ms=float(fields[9]),
            audio_lle_ms=float(fields[10]),
            vi_present_ms=float(fields[11]),
            resume_dispatch_ms=float(fields[12]),
            rsp_steps_gfx=int(fields[13]),
            rsp_steps_audio=int(fields[14]),
            **task_fields,
        )
        if pump.index != len(pumps):
            raise ValueError(
                f"pump sequence is not contiguous: expected index {len(pumps)}, got {pump.index}"
            )
        pumps.append(pump)
    if not pumps:
        raise ValueError(
            "no [pump-seq] rows found; set FN64_PUMP_CENSUS_SEQUENCE equal to "
            "FN64_PUMP_CENSUS_PUMPS"
        )
    return pumps


def population_means(
    frames: list[dict[str, float]], metrics: tuple[str, ...] = METRICS
) -> dict[str, float]:
    if not frames:
        return {"count": 0}
    return {
        "count": len(frames),
        "drawn_frame_ms": sum(frame["drawn_frame_ms"] for frame in frames) / len(frames),
        **{
            metric: sum(frame[metric] for frame in frames) / len(frames)
            for metric in metrics
        },
    }


def distribution(values: list[float]) -> dict[str, float]:
    return {
        "mean": sum(values) / len(values),
        "p50": percentile(values, 0.50),
        "p95": percentile(values, 0.95),
        "p99": percentile(values, 0.99),
        "max": max(values),
    }


def summarize(text: str) -> dict[str, object]:
    renderer_match = RENDERER_RE.search(text)
    if renderer_match is None:
        raise ValueError("pump census renderer identity is missing")
    renderer = renderer_match.group(1)
    pumps = parse_pumps(text)
    task_phase_available = pumps[0].task_completion_before is not None
    if any((pump.task_completion_before is not None) != task_phase_available for pump in pumps):
        raise ValueError("pump sequence mixes legacy 15-field and expanded 28-field rows")
    swap_indices = [pump.index for pump in pumps if pump.swapped]
    if len(swap_indices) < 2:
        raise ValueError("at least two post-warmup VI swaps are required")

    gaps = [current - previous for previous, current in zip(swap_indices, swap_indices[1:])]
    frames = []
    for previous, current in zip(swap_indices, swap_indices[1:]):
        span = pumps[previous + 1 : current + 1]
        frame = {
            "drawn_frame_ms": sum(pump.wall_ms for pump in span),
            **{
                metric: sum(getattr(pump, metric) for pump in span)
                for metric in METRICS
            },
        }
        if task_phase_available:
            frame.update(
                {
                    "task_completions": (
                        span[-1].task_completion_after
                        - span[0].task_completion_before
                    ),
                    **{
                        metric: sum(getattr(pump, metric) for pump in span)
                        for metric in TASK_PHASE_METRICS
                    },
                }
            )
        frames.append(frame)
    drawn_ms = [frame["drawn_frame_ms"] for frame in frames]
    gap_counts = Counter(gaps)
    over_budget = sum(value > BUDGET_MS for value in drawn_ms)
    within = [frame for frame in frames if frame["drawn_frame_ms"] <= BUDGET_MS]
    over = [frame for frame in frames if frame["drawn_frame_ms"] > BUDGET_MS]
    within_means = population_means(within)
    over_means = population_means(over)

    result = {
        "schema": "fn64.wm2000-swap-latency.v3",
        "renderer": renderer,
        "pumps": len(pumps),
        "swaps": len(swap_indices),
        "drawn_frames": len(drawn_ms),
        "swap_gap_histogram": {str(gap): gap_counts[gap] for gap in sorted(gap_counts)},
        "gap_two_fraction": gap_counts[2] / len(gaps),
        "budget_ms": BUDGET_MS,
        "drawn_frame_ms": {
            **distribution(drawn_ms),
        },
        "over_budget": {
            "count": over_budget,
            "fraction": over_budget / len(drawn_ms),
        },
        "drawn_frame_populations": {
            "within_budget_mean": within_means,
            "over_budget_mean": over_means,
            "over_minus_within": {
                metric: over_means[metric] - within_means[metric]
                for metric in ("drawn_frame_ms", *METRICS)
                if within and over
            },
        },
    }
    if task_phase_available:
        phase_metrics = ("task_completions", *TASK_PHASE_METRICS)
        result["task_cpu_phase_frames"] = {
            "available": True,
            "metrics": {
                metric: distribution([frame[metric] for frame in frames])
                for metric in phase_metrics
            },
            "within_budget_mean": population_means(within, phase_metrics),
            "over_budget_mean": population_means(over, phase_metrics),
            "over_minus_within": {
                metric: (
                    sum(frame[metric] for frame in over) / len(over)
                    - sum(frame[metric] for frame in within) / len(within)
                )
                for metric in phase_metrics
                if within and over
            },
        }
    else:
        result["task_cpu_phase_frames"] = {"available": False}
    return result


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
        sys.stdout.write(encoded)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
