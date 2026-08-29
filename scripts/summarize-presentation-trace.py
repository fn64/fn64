#!/usr/bin/env python3
"""Summarize fn64's joined host A/V presentation trace.

The reported residual compares two mappings of typed emulated time onto one
host epoch. A negative value means the VI present call returned earlier than
the audio anchor predicts playback for the same emulated cycle. It is not a
measurement of display scanout or acoustic output latency.
"""

from __future__ import annotations

import argparse
import bisect
import json
import statistics
from pathlib import Path
from typing import Any


SCHEMA = "fn64.host-presentation.v1"


def _integer(record: dict[str, Any], key: str) -> int:
    value = record.get(key)
    if not isinstance(value, int) or isinstance(value, bool):
        raise ValueError(f"{record.get('record', 'record')}.{key} must be an integer")
    return value


def load_trace(path: Path) -> tuple[dict[str, Any], list[dict[str, Any]]]:
    records = []
    with path.open(encoding="utf-8") as stream:
        for line_number, line in enumerate(stream, 1):
            try:
                record = json.loads(line)
            except json.JSONDecodeError as error:
                raise ValueError(f"line {line_number}: {error}") from error
            if not isinstance(record, dict):
                raise ValueError(f"line {line_number}: record must be an object")
            records.append(record)
    if len(records) < 2:
        raise ValueError("trace must contain header and end records")
    header, end = records[0], records[-1]
    if header.get("record") != "header" or header.get("schema") != SCHEMA:
        raise ValueError(f"expected {SCHEMA} header")
    if end.get("record") != "end":
        raise ValueError("trace is not sealed with an end record")
    data = records[1:-1]
    if _integer(end, "data_records") != len(data):
        raise ValueError("end.data_records does not match the sealed trace")
    if _integer(header, "emulated_hz") <= 0:
        raise ValueError("header.emulated_hz must be positive")
    return header, data


def _nearest_anchor(
    anchors: list[dict[str, Any]], cycles: list[int], cycle: int
) -> dict[str, Any]:
    index = bisect.bisect_left(cycles, cycle)
    candidates = anchors[max(0, index - 1) : min(len(anchors), index + 1)]
    return min(candidates, key=lambda anchor: abs(_integer(anchor, "emulated_cycle") - cycle))


def _percentile(values: list[float], percentile: float) -> float:
    ordered = sorted(values)
    index = max(0, min(len(ordered) - 1, int(len(ordered) * percentile + 0.999999) - 1))
    return ordered[index]


def _least_squares_rate(
    records: list[dict[str, Any]], cycle_key: str, host_ns_key: str, hz: int
) -> float | None:
    if len(records) < 2:
        return None
    points = [
        (
            _integer(record, cycle_key) / hz,
            _integer(record, host_ns_key) / 1_000_000_000,
        )
        for record in records
    ]
    mean_emulated = statistics.fmean(point[0] for point in points)
    mean_host = statistics.fmean(point[1] for point in points)
    variance = sum((emulated - mean_emulated) ** 2 for emulated, _ in points)
    if variance == 0:
        return None
    covariance = sum(
        (emulated - mean_emulated) * (host - mean_host)
        for emulated, host in points
    )
    return covariance / variance


def summarize(
    header: dict[str, Any], data: list[dict[str, Any]], tolerance_ms: float
) -> dict[str, Any]:
    hz = _integer(header, "emulated_hz")
    anchors = [record for record in data if record.get("record") == "audio_anchor"]
    fields = [record for record in data if record.get("record") == "vi_present"]
    if not anchors:
        raise ValueError("trace contains no complete audio anchors")
    if not fields:
        raise ValueError("trace contains no VI presents")
    anchors.sort(key=lambda record: _integer(record, "emulated_cycle"))
    anchor_cycles = [_integer(record, "emulated_cycle") for record in anchors]
    comparable_fields = [
        field
        for field in fields
        if _integer(field, "retrace_cycle") >= anchor_cycles[0]
    ]
    if not comparable_fields:
        raise ValueError("trace contains no VI presents at or after its first audio anchor")

    comparisons = []
    for field in comparable_fields:
        vi_cycle = _integer(field, "retrace_cycle")
        anchor = _nearest_anchor(anchors, anchor_cycles, vi_cycle)
        audio_offset_ns = _integer(anchor, "predicted_playback_host_ns") - (
            _integer(anchor, "emulated_cycle") * 1_000_000_000 / hz
        )
        video_offset_ns = _integer(field, "present_return_host_ns") - (
            vi_cycle * 1_000_000_000 / hz
        )
        comparisons.append(
            {
                "source_generation": _integer(field, "source_generation"),
                "retrace_cycle": vi_cycle,
                "swap_count": _integer(field, "swap_count"),
                "audio_generation": _integer(anchor, "generation"),
                "audio_dma_id": _integer(anchor, "dma_id"),
                "audio_offset_ms": audio_offset_ns / 1_000_000,
                "video_offset_ms": video_offset_ns / 1_000_000,
                "video_minus_audio_ms": (video_offset_ns - audio_offset_ns) / 1_000_000,
            }
        )

    residuals = [item["video_minus_audio_ms"] for item in comparisons]
    violating = next(
        (item for item in comparisons if abs(item["video_minus_audio_ms"]) > tolerance_ms),
        None,
    )
    overlap_start = max(
        _integer(anchors[0], "emulated_cycle"),
        _integer(fields[0], "retrace_cycle"),
    )
    overlap_end = min(
        _integer(anchors[-1], "emulated_cycle"),
        _integer(fields[-1], "retrace_cycle"),
    )
    pace_anchors = [
        anchor
        for anchor in anchors
        if overlap_start <= _integer(anchor, "emulated_cycle") <= overlap_end
    ]
    pace_fields = [
        field
        for field in fields
        if overlap_start <= _integer(field, "retrace_cycle") <= overlap_end
    ]
    audio_rate = _least_squares_rate(
        pace_anchors, "emulated_cycle", "predicted_playback_host_ns", hz
    )
    video_rate = _least_squares_rate(
        pace_fields, "retrace_cycle", "present_return_host_ns", hz
    )
    relative_pace = None
    if audio_rate is not None and video_rate is not None and audio_rate > 0:
        relative_pace = {
            "overlap_start_cycle": overlap_start,
            "overlap_end_cycle": overlap_end,
            "audio_samples": len(pace_anchors),
            "video_samples": len(pace_fields),
            "audio_host_seconds_per_emulated_second": audio_rate,
            "video_host_seconds_per_emulated_second": video_rate,
            "video_vs_audio_rate_ppm": (video_rate / audio_rate - 1) * 1_000_000,
            "video_minus_audio_drift_ms_per_minute": (video_rate - audio_rate)
            * 60_000,
        }
    return {
        "schema": SCHEMA,
        "trace_id": header.get("trace_id"),
        "audio_anchors": len(anchors),
        "vi_presents": len(fields),
        "comparisons": len(comparisons),
        "vi_before_first_audio_anchor": len(fields) - len(comparable_fields),
        "tolerance_ms": tolerance_ms,
        "video_minus_audio_ms": {
            "median": statistics.median(residuals),
            "p05": _percentile(residuals, 0.05),
            "p95": _percentile(residuals, 0.95),
            "minimum": min(residuals),
            "maximum": max(residuals),
        },
        "relative_pace": relative_pace,
        "first_outside_tolerance": violating,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("trace", type=Path)
    parser.add_argument("--tolerance-ms", type=float, default=5.0)
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()
    if args.tolerance_ms < 0:
        parser.error("--tolerance-ms must be nonnegative")
    try:
        header, data = load_trace(args.trace)
        summary = summarize(header, data, args.tolerance_ms)
    except (OSError, ValueError) as error:
        parser.error(str(error))
    if args.json:
        print(json.dumps(summary, sort_keys=True))
    else:
        phase = summary["video_minus_audio_ms"]
        print(
            f"trace={summary['trace_id']} anchors={summary['audio_anchors']} "
            f"vi={summary['vi_presents']} comparisons={summary['comparisons']}"
        )
        print(
            "video-minus-audio ms: "
            f"median={phase['median']:.3f} p05={phase['p05']:.3f} "
            f"p95={phase['p95']:.3f} range={phase['minimum']:.3f}..{phase['maximum']:.3f}"
        )
        pace = summary["relative_pace"]
        if pace is None:
            print("relative pace: unavailable (need two audio and two video samples in overlap)")
        else:
            print(
                "relative pace: "
                f"video_vs_audio={pace['video_vs_audio_rate_ppm']:+.1f} ppm "
                f"phase_drift={pace['video_minus_audio_drift_ms_per_minute']:+.3f} ms/min "
                f"(audio_n={pace['audio_samples']} video_n={pace['video_samples']}; "
                "negative means video pulls farther ahead)"
            )
        first = summary["first_outside_tolerance"]
        if first is None:
            print(f"all fields within {summary['tolerance_ms']:.3f} ms")
        else:
            print(
                f"first outside {summary['tolerance_ms']:.3f} ms: "
                f"source_generation={first['source_generation']} "
                f"retrace_cycle={first['retrace_cycle']} swap={first['swap_count']} "
                f"audio_generation={first['audio_generation']} dma={first['audio_dma_id']} "
                f"residual={first['video_minus_audio_ms']:.3f} ms"
            )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
