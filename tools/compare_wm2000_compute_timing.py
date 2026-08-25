#!/usr/bin/env python3
"""Compare compute-chain timing and task-census totals from two WM2000 logs."""

from __future__ import annotations

import argparse
import json
import math
import re
from dataclasses import dataclass, field
from pathlib import Path
from typing import Iterable


NUMBER = r"-?(?:\d+(?:\.\d*)?|\.\d+)"
KEY_VALUE_RE = re.compile(rf"([a-zA-Z_][a-zA-Z0-9_]*)=({NUMBER})")
PHASE_RE = re.compile(
    rf"^\[task-batch-phase\]\s+(?P<label>[a-z+-]+)\s+(?P<total>{NUMBER}) ms\s+"
    rf"(?P<per_task>{NUMBER}) ms/task$"
)

CHAIN_FIELDS = (
    "dispatches",
    "draws",
    "pixels",
    "prepare_ms",
    "resources_ms",
    "uploads_ms",
    "bind_groups_ms",
    "encode_ms",
    "submit_ms",
    "wait_ms",
    "gpu_map_ms",
    "status_map_ms",
    "target_map_ms",
    "total_ms",
)


def key_values(line: str) -> dict[str, float]:
    return {key: float(value) for key, value in KEY_VALUE_RE.findall(line)}


@dataclass
class ParsedLog:
    path: Path
    chains: list[dict[str, float]] = field(default_factory=list)
    gpu_chains: list[dict[str, float]] = field(default_factory=list)
    task_compute: dict[str, float] | None = None
    task_batch: dict[str, float] | None = None
    task_batch_phases: dict[str, float] = field(default_factory=dict)


def parse_log(path: Path) -> ParsedLog:
    parsed = ParsedLog(path)
    for line_number, raw in enumerate(path.read_text(errors="strict").splitlines(), 1):
        line = raw.strip()
        if line.startswith("[compute-chain-timing]"):
            values = key_values(line)
            missing = [field for field in CHAIN_FIELDS if field not in values]
            if missing:
                raise ValueError(
                    f"{path}:{line_number}: compute-chain-timing missing {', '.join(missing)}"
                )
            parsed.chains.append(values)
        elif line.startswith("[compute-gpu-timing]"):
            values = key_values(line)
            required = ("semantic_dispatches", "passes", "valid_sum_ms", "invalid_dispatches")
            missing = [field for field in required if field not in values]
            if missing:
                raise ValueError(
                    f"{path}:{line_number}: compute-gpu-timing missing {', '.join(missing)}"
                )
            span = re.search(rf"\bspan_ms=Some\(({NUMBER})\)", line)
            values["span_ms"] = float(span.group(1)) if span else math.nan
            parsed.gpu_chains.append(values)
        elif line.startswith("[task-compute-census] tasks="):
            parsed.task_compute = key_values(line)
        elif line.startswith("[task-batch-phase] tasks="):
            parsed.task_batch = key_values(line)
        else:
            phase = PHASE_RE.match(line)
            if phase:
                parsed.task_batch_phases[phase.group("label")] = float(phase.group("total"))

    if not parsed.chains:
        raise ValueError(f"{path}: no [compute-chain-timing] records")
    if parsed.gpu_chains and len(parsed.gpu_chains) != len(parsed.chains):
        raise ValueError(
            f"{path}: {len(parsed.chains)} compute-chain records but "
            f"{len(parsed.gpu_chains)} GPU timing records"
        )
    return parsed


def total(records: Iterable[dict[str, float]], field: str) -> float:
    return sum(record[field] for record in records)


def divide(value: float, denominator: float | None) -> float | None:
    if denominator is None or denominator == 0:
        return None
    return value / denominator


def summarize(parsed: ParsedLog) -> dict[str, float | None]:
    chain_count = float(len(parsed.chains))
    metrics: dict[str, float | None] = {
        "chains": chain_count,
    }
    for field in CHAIN_FIELDS:
        metrics[f"chain_total.{field}"] = total(parsed.chains, field)
        metrics[f"per_chain.{field}"] = total(parsed.chains, field) / chain_count

    host_fields = (
        "prepare_ms",
        "resources_ms",
        "uploads_ms",
        "bind_groups_ms",
        "encode_ms",
        "submit_ms",
    )
    host_total = sum(float(metrics[f"chain_total.{field}"]) for field in host_fields)
    metrics["chain_total.host_prep_ms"] = host_total
    metrics["per_chain.host_prep_ms"] = host_total / chain_count

    if parsed.gpu_chains:
        valid_spans = [
            record["span_ms"]
            for record in parsed.gpu_chains
            if not math.isnan(record["span_ms"])
        ]
        metrics.update(
            {
                "gpu_timed_chains": float(len(parsed.gpu_chains)),
                "gpu_valid_spans": float(len(valid_spans)),
                "gpu_total.passes": total(parsed.gpu_chains, "passes"),
                "gpu_total.valid_sum_ms": total(parsed.gpu_chains, "valid_sum_ms"),
                "gpu_total.span_ms": sum(valid_spans),
                "gpu_total.invalid_dispatches": total(parsed.gpu_chains, "invalid_dispatches"),
                "per_chain.passes": total(parsed.gpu_chains, "passes") / chain_count,
                "per_chain.gpu_valid_sum_ms": (
                    total(parsed.gpu_chains, "valid_sum_ms") / chain_count
                ),
                "per_valid_span.gpu_span_ms": divide(sum(valid_spans), float(len(valid_spans))),
            }
        )
    else:
        metrics.update(
            {
                "gpu_timed_chains": 0.0,
                "gpu_valid_spans": 0.0,
                "gpu_total.passes": None,
                "gpu_total.valid_sum_ms": None,
                "gpu_total.span_ms": None,
                "gpu_total.invalid_dispatches": None,
                "per_chain.passes": None,
                "per_chain.gpu_valid_sum_ms": None,
                "per_valid_span.gpu_span_ms": None,
            }
        )

    census = parsed.task_compute
    for field in (
        "tasks",
        "members",
        "compute_segments",
        "compute_members",
        "cpu_members",
        "compute_total_ms",
        "timed_cpu_members",
        "timed_cpu_total_ms",
    ):
        metrics[f"task_census.{field}"] = census.get(field) if census else None

    # The production task path retains one target checkpoint per compute
    # member so each original packet can publish independently. The census's
    # compute_members is therefore the observable checkpoint denominator;
    # unlike inferred dispatch counts, it survives pass fusion.
    checkpoints = census.get("compute_members") if census else None
    metrics["checkpoints"] = checkpoints
    metrics["per_checkpoint.passes"] = divide(
        metrics["gpu_total.passes"] if isinstance(metrics["gpu_total.passes"], float) else 0.0,
        checkpoints,
    ) if metrics["gpu_total.passes"] is not None else None
    for field in (
        "host_prep_ms",
        "wait_ms",
        "gpu_map_ms",
        "status_map_ms",
        "target_map_ms",
        "total_ms",
    ):
        metrics[f"per_checkpoint.{field}"] = divide(
            float(metrics[f"chain_total.{field}"]), checkpoints
        )
    metrics["per_checkpoint.gpu_valid_sum_ms"] = divide(
        float(metrics["gpu_total.valid_sum_ms"]), checkpoints
    ) if metrics["gpu_total.valid_sum_ms"] is not None else None

    if parsed.task_batch:
        for field in ("tasks", "members", "total_ms"):
            metrics[f"task_batch.{field}"] = parsed.task_batch.get(field)
    for label, value in sorted(parsed.task_batch_phases.items()):
        metrics[f"task_batch_phase.{label}_ms"] = value
    return metrics


def comparison(
    baseline: dict[str, float | None], candidate: dict[str, float | None]
) -> list[dict[str, float | str | None]]:
    rows = []
    for metric in sorted(set(baseline) | set(candidate)):
        before = baseline.get(metric)
        after = candidate.get(metric)
        delta = None if before is None or after is None else after - before
        percent = None if delta is None or before == 0 else delta / before * 100.0
        rows.append(
            {
                "metric": metric,
                "baseline": before,
                "candidate": after,
                "delta": delta,
                "delta_percent": percent,
            }
        )
    return rows


def format_value(value: float | None) -> str:
    return "n/a" if value is None else f"{value:.3f}"


def print_table(rows: list[dict[str, float | str | None]]) -> None:
    print(f"{'metric':<43} {'baseline':>12} {'candidate':>12} {'delta':>12} {'delta %':>10}")
    for row in rows:
        percent = row["delta_percent"]
        percent_text = "n/a" if percent is None else f"{float(percent):+.2f}%"
        print(
            f"{str(row['metric']):<43} "
            f"{format_value(row['baseline']):>12} "
            f"{format_value(row['candidate']):>12} "
            f"{format_value(row['delta']):>12} "
            f"{percent_text:>10}"
        )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("baseline", type=Path)
    parser.add_argument("candidate", type=Path)
    parser.add_argument("--json", action="store_true", help="emit machine-readable JSON")
    args = parser.parse_args()

    baseline = summarize(parse_log(args.baseline))
    candidate = summarize(parse_log(args.candidate))
    rows = comparison(baseline, candidate)
    if args.json:
        print(
            json.dumps(
                {
                    "schema": "fn64.wm2000-compute-timing-comparison.v1",
                    "baseline": str(args.baseline),
                    "candidate": str(args.candidate),
                    "metrics": rows,
                },
                indent=2,
                sort_keys=True,
                allow_nan=False,
            )
        )
    else:
        print_table(rows)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
