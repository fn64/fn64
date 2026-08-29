#!/usr/bin/env python3
"""Summarize WM2000 swap-to-swap latency from a pump-census sequence dump."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import pathlib
import re
import sys
from collections import Counter
from dataclasses import dataclass


SEQUENCE_PREFIX = "[pump-seq] "
SEQUENCE_SCHEMA_LINE = "[pump-census] sequence schema: fn64.pump-sequence.v2"
WALL_CADENCE_PREFIX = "[wall-cadence-seq] "
WALL_SWAP_PREFIX = "[wall-swap-seq] "
PRESENT_DEPENDENCY_PREFIX = "[present-dependency-seq] "
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
    task_rdp_front_half_ms: float | None = None
    session_plan_ms: float | None = None
    session_finalize_ms: float | None = None
    session_execute_ms: float | None = None
    session_commit_ms: float | None = None
    task_batch_total_ms: float | None = None
    task_batch_setup_ms: float | None = None
    task_batch_plan_bind_ms: float | None = None
    task_batch_guest_reads_ms: float | None = None
    task_batch_staged_writes_ms: float | None = None
    task_batch_copyback_ms: float | None = None
    task_batch_publication_ms: float | None = None
    task_batch_tasks: int | None = None


@dataclass(frozen=True)
class WallCadence:
    index: int
    pump_start_ms: float
    scheduled_deadline_ms: float
    interval_ms: float
    start_debt_ms: float
    wake_overshoot_ms: float
    reanchored: bool
    prior_pump_ms: float
    prior_present_ms: float
    intended_wait_ms: float
    outside_residual_ms: float


@dataclass(frozen=True)
class PresentDependency:
    pump: int
    mode: str
    overscan: int
    zoom_fill: bool
    generation: int
    invalidations: int
    probe_ns: int
    dependency: str
    reason: str | None
    start: int | None
    src_stride: int | None
    dst_width: int | None
    dst_height: int | None
    blanked: bool | None
    bytes: int | None
    fnv_digest: str | None
    sha256: str | None
    exact_hit: bool
    disposition: str

    def canonical_identity(self) -> tuple[object, ...]:
        policy = (self.overscan, self.zoom_fill)
        if self.dependency == "Uncacheable":
            return (*policy, "Uncacheable", self.reason)
        return (*policy,
            "Cacheable",
            self.start,
            self.src_stride,
            self.dst_width,
            self.dst_height,
            self.blanked,
            self.bytes,
            self.sha256,
        )


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
    "task_rdp_front_half_ms",
)

ABI_PHASE_METRICS = (
    "session_plan_ms",
    "session_finalize_ms",
    "session_execute_ms",
    "session_commit_ms",
    "task_batch_total_ms",
    "task_batch_setup_ms",
    "task_batch_plan_bind_ms",
    "task_batch_guest_reads_ms",
    "task_batch_staged_writes_ms",
    "task_batch_copyback_ms",
    "task_batch_publication_ms",
)

ABI_DERIVED_METRICS = (
    "execute_outer_ms",
    "post_execute_outer_ms",
    "post_execute_accounted_ms",
    "post_execute_unattributed_ms",
    "pre_execute_accounted_ms",
    "front_half_unattributed_ms",
)


def percentile(values: list[float], fraction: float) -> float:
    if not values:
        raise ValueError("a percentile requires at least one value")
    ordered = sorted(values)
    rank = max(1, math.ceil(fraction * len(ordered)))
    return ordered[rank - 1]


def parse_pumps(text: str) -> list[Pump]:
    pumps: list[Pump] = []
    sequence_schema_present = SEQUENCE_SCHEMA_LINE in text.splitlines()
    for line in text.splitlines():
        if not line.startswith(SEQUENCE_PREFIX):
            continue
        fields = line[len(SEQUENCE_PREFIX) :].split(",")
        if len(fields) not in (15, 28, 40):
            raise ValueError(
                f"pump sequence row has {len(fields)} fields, expected 15, 28, or 40"
            )
        if len(fields) >= 28 and not sequence_schema_present:
            raise ValueError(
                f"expanded pump sequence requires {SEQUENCE_SCHEMA_LINE}"
            )
        task_fields: dict[str, int | float | None] = {}
        if len(fields) >= 28:
            task_fields = {
                "task_completion_before": int(fields[15]),
                "task_completion_after": int(fields[16]),
                **{
                    metric: float(value)
                    for metric, value in zip(TASK_PHASE_METRICS, fields[17:])
                },
            }
        if len(fields) == 40:
            task_fields.update(
                {
                    metric: float(value)
                    for metric, value in zip(ABI_PHASE_METRICS, fields[28:])
                }
            )
            task_fields["task_batch_tasks"] = int(fields[39])
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


def parse_wall_cadence(text: str, pumps: list[Pump]) -> list[WallCadence]:
    samples: list[WallCadence] = []
    for line in text.splitlines():
        if not line.startswith(WALL_CADENCE_PREFIX):
            continue
        fields = line[len(WALL_CADENCE_PREFIX) :].split(",")
        if len(fields) != 11:
            raise ValueError(
                f"wall cadence row has {len(fields)} fields, expected 11"
            )
        index = int(fields[0])
        if index < 0:
            raise ValueError("wall cadence index must be non-negative")
        if fields[6] not in ("0", "1"):
            raise ValueError("wall cadence reanchored field must be 0 or 1")
        sample = WallCadence(
            index=index,
            pump_start_ms=float(fields[1]),
            scheduled_deadline_ms=float(fields[2]),
            interval_ms=float(fields[3]),
            start_debt_ms=float(fields[4]),
            wake_overshoot_ms=float(fields[5]),
            reanchored=fields[6] == "1",
            prior_pump_ms=float(fields[7]),
            prior_present_ms=float(fields[8]),
            intended_wait_ms=float(fields[9]),
            outside_residual_ms=float(fields[10]),
        )
        if sample.index >= len(pumps):
            raise ValueError(
                f"wall cadence index {sample.index} has no matching pump row"
            )
        if samples and sample.index <= samples[-1].index:
            raise ValueError("wall cadence rows are not in strictly increasing pump order")
        samples.append(sample)
    return samples


def parse_wall_swaps(text: str, pumps: list[Pump]) -> list[tuple[int, float]]:
    samples: list[tuple[int, float]] = []
    for line in text.splitlines():
        if not line.startswith(WALL_SWAP_PREFIX):
            continue
        fields = line[len(WALL_SWAP_PREFIX) :].split(",")
        if len(fields) != 2:
            raise ValueError(f"wall swap row has {len(fields)} fields, expected 2")
        index, duration_ms = int(fields[0]), float(fields[1])
        if index < 0:
            raise ValueError("wall swap index must be non-negative")
        if index >= len(pumps) or not pumps[index].swapped:
            raise ValueError(f"wall swap index {index} does not name a swapped pump")
        if samples and index <= samples[-1][0]:
            raise ValueError("wall swap rows are not in strictly increasing pump order")
        samples.append((index, duration_ms))
    return samples


def parse_present_dependencies(
    text: str, expected_count: int | None = None
) -> list[PresentDependency]:
    samples: list[PresentDependency] = []
    for line in text.splitlines():
        if not line.startswith(PRESENT_DEPENDENCY_PREFIX):
            continue
        raw_fields = line[len(PRESENT_DEPENDENCY_PREFIX) :].split()
        if any(field.count("=") != 1 for field in raw_fields):
            raise ValueError("present dependency row fields must be canonical key=value tokens")
        fields = dict(field.split("=", 1) for field in raw_fields)
        if len(fields) != len(raw_fields):
            raise ValueError("present dependency row contains a duplicate field")
        common = {
            "pump", "mode", "dependency", "overscan", "zoom_fill",
            "generation", "invalidations", "probe_ns", "exact_hit", "disposition",
        }
        missing = common - fields.keys()
        if missing:
            raise ValueError(
                "present dependency row is missing " + ", ".join(sorted(missing))
            )
        if fields["mode"] not in ("Observe", "Suppress"):
            raise ValueError("present dependency mode must be Observe or Suppress")
        if fields["exact_hit"] not in ("0", "1"):
            raise ValueError("present dependency exact_hit must be 0 or 1")
        if fields["disposition"] not in ("Redraw", "Suppress"):
            raise ValueError("present dependency disposition must be Redraw or Suppress")
        pump = int(fields["pump"])
        if pump != len(samples):
            raise ValueError(
                f"present dependency rows are not contiguous: expected pump {len(samples)}, got {pump}"
            )
        exact_hit = fields["exact_hit"] == "1"
        if fields["zoom_fill"] not in ("0", "1"):
            raise ValueError("present dependency zoom_fill must be 0 or 1")
        diagnostics = {
            "overscan": int(fields["overscan"]),
            "zoom_fill": fields["zoom_fill"] == "1",
            "generation": int(fields["generation"]),
            "invalidations": int(fields["invalidations"]),
            "probe_ns": int(fields["probe_ns"]),
        }
        if any(value < 0 for key, value in diagnostics.items() if key != "zoom_fill"):
            raise ValueError("present dependency policy/diagnostic fields must be non-negative")
        if fields["dependency"] == "Cacheable":
            keys = common | {
                "start",
                "src_stride",
                "dst_width",
                "dst_height",
                "blanked",
                "bytes",
                "fnv_digest",
                "sha256",
            }
            if fields.keys() != keys:
                raise ValueError("cacheable present dependency row has a non-canonical field set")
            if fields["blanked"] not in ("0", "1"):
                raise ValueError("present dependency blanked must be 0 or 1")
            if not re.fullmatch(r"[0-9a-f]{16}", fields["fnv_digest"]):
                raise ValueError("present dependency FNV digest must be 16 lowercase hex digits")
            if not re.fullmatch(r"[0-9a-f]{64}", fields["sha256"]):
                raise ValueError("present dependency SHA-256 must be 64 lowercase hex digits")
            sample = PresentDependency(
                pump=pump,
                mode=fields["mode"],
                **diagnostics,
                dependency="Cacheable",
                reason=None,
                start=int(fields["start"]),
                src_stride=int(fields["src_stride"]),
                dst_width=int(fields["dst_width"]),
                dst_height=int(fields["dst_height"]),
                blanked=fields["blanked"] == "1",
                bytes=int(fields["bytes"]),
                fnv_digest=fields["fnv_digest"],
                sha256=fields["sha256"],
                exact_hit=exact_hit,
                disposition=fields["disposition"],
            )
            if any(
                value is not None and value < 0
                for value in (
                    sample.start,
                    sample.src_stride,
                    sample.dst_width,
                    sample.dst_height,
                    sample.bytes,
                )
            ):
                raise ValueError("present dependency numeric identity fields must be non-negative")
        elif fields["dependency"] == "Uncacheable":
            if fields.keys() != common | {"reason"}:
                raise ValueError("uncacheable present dependency row has a non-canonical field set")
            if exact_hit or fields["disposition"] != "Redraw":
                raise ValueError("uncacheable present dependency must miss and redraw")
            if fields["reason"] not in (
                "Overlay",
                "FrameTrip",
                "FrameDump",
                "MissingFramebuffer",
                "UnavailableFramebuffer",
                "OutsideRdram",
                "UnalignedFramebuffer",
            ):
                raise ValueError("uncacheable present dependency reason is unknown")
            sample = PresentDependency(
                pump=pump,
                mode=fields["mode"],
                **diagnostics,
                dependency="Uncacheable",
                reason=fields["reason"],
                start=None,
                src_stride=None,
                dst_width=None,
                dst_height=None,
                blanked=None,
                bytes=None,
                fnv_digest=None,
                sha256=None,
                exact_hit=False,
                disposition="Redraw",
            )
        else:
            raise ValueError("present dependency must be Cacheable or Uncacheable")
        if sample.mode == "Observe" and sample.disposition != "Redraw":
            raise ValueError("Observe present dependency cannot suppress redraw")
        if sample.disposition == "Suppress" and not sample.exact_hit:
            raise ValueError("only an exact hit may suppress redraw")
        if sample.mode == "Suppress" and sample.exact_hit != (
            sample.disposition == "Suppress"
        ):
            raise ValueError("Suppress present dependency disposition disagrees with exact_hit")
        samples.append(sample)
    if expected_count is not None and len(samples) != expected_count:
        raise ValueError(
            f"present dependency receipt requires {expected_count} contiguous rows, got {len(samples)}"
        )
    if samples and any(sample.mode != samples[0].mode for sample in samples):
        raise ValueError("present dependency receipt mixes Observe and Suppress rows")
    return samples


def present_identity_sha256(samples: list[PresentDependency]) -> str:
    wire = json.dumps(
        [sample.canonical_identity() for sample in samples],
        ensure_ascii=True,
        separators=(",", ":"),
    ).encode()
    return hashlib.sha256(wire).hexdigest()


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
    wall_cadence = parse_wall_cadence(text, pumps)
    wall_swaps = parse_wall_swaps(text, pumps)
    present_dependencies = parse_present_dependencies(text)
    if present_dependencies and len(present_dependencies) != len(pumps):
        raise ValueError(
            "present dependency rows must cover every measured pump, including the final pump"
        )
    task_phase_available = pumps[0].task_completion_before is not None
    abi_phase_available = pumps[0].session_plan_ms is not None
    if any((pump.task_completion_before is not None) != task_phase_available for pump in pumps):
        raise ValueError("pump sequence mixes rows with and without task phase fields")
    if any((pump.session_plan_ms is not None) != abi_phase_available for pump in pumps):
        raise ValueError("pump sequence mixes rows with and without ABI phase fields")
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
        if abi_phase_available:
            frame.update(
                {
                    metric: sum(getattr(pump, metric) for pump in span)
                    for metric in ABI_PHASE_METRICS
                }
            )
            frame["task_batch_tasks"] = sum(pump.task_batch_tasks for pump in span)
            frame["abi_identity_closed"] = float(
                frame["task_batch_tasks"] == frame["task_completions"]
            )
            frame.update(
                {
                    "execute_outer_ms": max(
                        frame["session_execute_ms"] - frame["task_renderer_work_ms"], 0.0
                    ),
                    "post_execute_outer_ms": max(
                        frame["task_envelope_ms"] - frame["session_execute_ms"], 0.0
                    ),
                }
            )
            frame["post_execute_accounted_ms"] = sum(
                frame[metric]
                for metric in (
                    "task_batch_staged_writes_ms",
                    "session_commit_ms",
                    "task_batch_copyback_ms",
                    "task_batch_publication_ms",
                )
            )
            frame["post_execute_unattributed_ms"] = max(
                frame["post_execute_outer_ms"] - frame["post_execute_accounted_ms"], 0.0
            )
            frame["pre_execute_accounted_ms"] = sum(
                frame[metric]
                for metric in (
                    "task_batch_setup_ms",
                    "task_batch_plan_bind_ms",
                    "task_batch_guest_reads_ms",
                    "session_plan_ms",
                    "session_finalize_ms",
                )
            )
            frame["front_half_unattributed_ms"] = max(
                frame["task_rdp_front_half_ms"]
                - frame["pre_execute_accounted_ms"],
                0.0,
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
        "schema": "fn64.wm2000-swap-latency.v6",
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
    if abi_phase_available:
        result["abi_task_phase_frames"] = {
            "available": True,
            "metrics": {
                metric: distribution([frame[metric] for frame in frames])
                for metric in (*ABI_PHASE_METRICS, *ABI_DERIVED_METRICS)
            },
            "identity_closed_frames": sum(frame["abi_identity_closed"] for frame in frames),
            "identity_mismatch_frames": sum(
                not frame["abi_identity_closed"] for frame in frames
            ),
        }
    else:
        result["abi_task_phase_frames"] = {"available": False}
    if wall_cadence:
        swap_wall = [duration_ms for _, duration_ms in wall_swaps]
        result["wall_cadence"] = {
            "available": True,
            "completed_intervals": len(wall_cadence),
            "swap_intervals": len(swap_wall),
            "reanchors": sum(sample.reanchored for sample in wall_cadence),
            "totals_ms": {
                metric: sum(getattr(sample, metric) for sample in wall_cadence)
                for metric in (
                    "interval_ms",
                    "prior_pump_ms",
                    "prior_present_ms",
                    "intended_wait_ms",
                    "outside_residual_ms",
                    "start_debt_ms",
                    "wake_overshoot_ms",
                )
            },
            "distributions_ms": {
                metric: distribution(
                    [getattr(sample, metric) for sample in wall_cadence]
                )
                for metric in (
                    "interval_ms",
                    "start_debt_ms",
                    "wake_overshoot_ms",
                    "outside_residual_ms",
                )
            },
            "swap_to_swap_ms": distribution(swap_wall) if swap_wall else None,
        }
    else:
        result["wall_cadence"] = {"available": False}
    if present_dependencies:
        result["present_dependencies"] = {
            "available": True,
            "receipts": len(present_dependencies),
            "mode": present_dependencies[0].mode,
            "cacheable": sum(
                sample.dependency == "Cacheable" for sample in present_dependencies
            ),
            "uncacheable": sum(
                sample.dependency == "Uncacheable" for sample in present_dependencies
            ),
            "exact_hits": sum(sample.exact_hit for sample in present_dependencies),
            "suppressed": sum(
                sample.disposition == "Suppress" for sample in present_dependencies
            ),
            "canonical_identity_sha256": present_identity_sha256(present_dependencies),
            "probe_total_ms": sum(sample.probe_ns for sample in present_dependencies) / 1e6,
            "probe_ms": distribution(
                [sample.probe_ns / 1e6 for sample in present_dependencies]
            ),
        }
    else:
        result["present_dependencies"] = {"available": False}
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
