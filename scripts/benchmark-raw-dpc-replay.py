#!/usr/bin/env python3
"""Counterbalanced, identity-gated raw-DPC replay measurements."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import re
import statistics
import subprocess
import sys
import time


SCHEMA = "fn64.raw-dpc-replay-comparison.v1"
RESERVED_ENV = {
    "FN64_RAW_DPC_REPLAY_PACKET",
    "FN64_RAW_DPC_REPLAY_WINDOW",
    "FN64_RAW_DPC_REPLAY_TASK_BATCH",
    "FN64_RAW_DPC_REPLAY_WARMUP",
    "FN64_RAW_DPC_REPLAY_REPEAT",
}
ENV_NAME = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")
IDENTITY_KEYS = {
    "selected_window_packets",
    "replay_packets",
    "combined_window",
    "task_batch_window",
    "task_batches",
    "terminal_stream_bytes",
    "warmup",
    "repeat",
    "committed_fnv1a",
    "postimage_sha256",
}
METRIC_RE = re.compile(
    r"^\s*(\w+)\s+mean_ms=([0-9.]+)\s+p50_ms=([0-9.]+)\s+"
    r"p95_ms=([0-9.]+)\s+p99_ms=([0-9.]+)\s+max_ms=([0-9.]+)\s*$"
)


class MeasurementError(RuntimeError):
    pass


def sha256_file(path: Path) -> tuple[str, int]:
    digest = hashlib.sha256()
    size = 0
    with path.open("rb") as source:
        while chunk := source.read(1024 * 1024):
            digest.update(chunk)
            size += len(chunk)
    return digest.hexdigest(), size


def stable_file_receipt(path: Path, executable: bool = False) -> dict[str, object]:
    before = path.stat()
    if not path.is_file() or path.is_symlink():
        raise MeasurementError("preflight requires a regular, non-symlink file")
    if executable and not os.access(path, os.X_OK):
        raise MeasurementError("preflight binary is not executable")
    digest, size = sha256_file(path)
    after = path.stat()
    if (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns) != (
        after.st_dev,
        after.st_ino,
        after.st_size,
        after.st_mtime_ns,
    ):
        raise MeasurementError("preflight file changed while it was hashed")
    return {"sha256": digest, "size_bytes": size}


def stream_set_receipt(path: Path) -> dict[str, object]:
    if path.is_file():
        files = [path]
    elif path.is_dir():
        files = sorted(
            child
            for child in path.iterdir()
            if child.is_file()
            and child.name.startswith("raw-dpc-")
            and child.name.endswith("-xbus.bin")
        )
    else:
        raise MeasurementError("stream input is neither a file nor a directory")
    if not files:
        raise MeasurementError("stream input contains no replay streams")
    aggregate = hashlib.sha256()
    total = 0
    for index, child in enumerate(files):
        receipt = stable_file_receipt(child)
        total += int(receipt["size_bytes"])
        aggregate.update(index.to_bytes(8, "big"))
        aggregate.update(int(receipt["size_bytes"]).to_bytes(8, "big"))
        aggregate.update(bytes.fromhex(str(receipt["sha256"])))
    return {
        "sha256": aggregate.hexdigest(),
        "file_count": len(files),
        "size_bytes": total,
    }


def parse_assignments(values: list[str]) -> dict[str, str]:
    result: dict[str, str] = {}
    for value in values:
        name, separator, setting = value.partition("=")
        if not separator or not ENV_NAME.fullmatch(name):
            raise MeasurementError("lane environment must use NAME=VALUE")
        if name in RESERVED_ENV:
            raise MeasurementError("lane environment may not replace replay-control variables")
        if name in result:
            raise MeasurementError("lane environment names must be unique")
        result[name] = setting
    return result


def environment_digest(values: dict[str, str]) -> str:
    encoded = json.dumps(values, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(encoded).hexdigest()


def quiet_machine(max_load_one: float) -> dict[str, float]:
    process = subprocess.run(
        ["ps", "-Ao", "comm="], capture_output=True, text=True, check=True
    )
    heavy = [
        line
        for line in process.stdout.splitlines()
        if Path(line.strip()).name in {"cargo", "rustc"}
    ]
    if heavy:
        raise MeasurementError(
            f"quiet-machine preflight found {len(heavy)} cargo/rustc process(es)"
        )
    load_one = os.getloadavg()[0]
    if load_one > max_load_one:
        raise MeasurementError(
            f"quiet-machine preflight load {load_one:.3f} exceeds {max_load_one:.3f}"
        )
    return {"load_one": round(load_one, 3), "max_load_one": max_load_one}


def parse_output(output: str) -> tuple[dict[str, str], dict[str, dict[str, float]]]:
    identities = []
    metrics: dict[str, dict[str, float]] = {}
    for line in output.splitlines():
        fields = dict(item.split("=", 1) for item in line.split() if "=" in item)
        if IDENTITY_KEYS.issubset(fields):
            identities.append({key: fields[key] for key in sorted(IDENTITY_KEYS)})
        match = METRIC_RE.match(line)
        if match:
            name = match.group(1)
            values = [float(value) for value in match.groups()[1:]]
            if any(not math.isfinite(value) or value < 0 for value in values):
                raise MeasurementError(f"metric {name} is not finite and nonnegative")
            metrics[name] = dict(zip(("mean_ms", "p50_ms", "p95_ms", "p99_ms", "max_ms"), values))
    if len(identities) != 1:
        raise MeasurementError(f"expected one replay identity line, found {len(identities)}")
    for required in ("execute", "total"):
        if required not in metrics:
            raise MeasurementError(f"replay output omitted {required} metric")
    return identities[0], metrics


def canonical_receipt(document: dict[str, object]) -> dict[str, object]:
    encoded = json.dumps(document, sort_keys=True, separators=(",", ":")).encode()
    result = dict(document)
    result["receipt_sha256"] = hashlib.sha256(encoded).hexdigest()
    return result


def write_summary(output_dir: Path, document: dict[str, object]) -> None:
    summary = canonical_receipt(document)
    destination = output_dir / "summary.json"
    destination.write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n")


def lane_summary(legs: list[dict[str, object]], lane: str, metric: str) -> dict[str, float]:
    values = [
        float(leg["metrics"][metric]["mean_ms"])
        for leg in legs
        if leg["lane"] == lane and leg["phase"] == "timing"
    ]
    return {"mean_ms": statistics.fmean(values), "median_ms": statistics.median(values)}


def comparison(legs: list[dict[str, object]], metric: str) -> dict[str, object]:
    by_pair: dict[int, dict[str, float]] = {}
    for leg in legs:
        if leg["phase"] != "timing":
            continue
        by_pair.setdefault(int(leg["pair"]), {})[str(leg["lane"])] = float(
            leg["metrics"][metric]["mean_ms"]
        )
    deltas = [pair["candidate"] - pair["control"] for _, pair in sorted(by_pair.items())]
    control = lane_summary(legs, "control", metric)
    candidate = lane_summary(legs, "candidate", metric)
    return {
        "metric": metric,
        "pair_candidate_minus_control_ms": deltas,
        "control": control,
        "candidate": candidate,
        "candidate_minus_control_mean_ms": candidate["mean_ms"] - control["mean_ms"],
    }


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument("--control-bin", required=True, type=Path)
    result.add_argument("--candidate-bin", required=True, type=Path)
    result.add_argument("--streams", required=True, type=Path)
    result.add_argument("--rdram", required=True, type=Path)
    result.add_argument("--output-dir", required=True, type=Path)
    result.add_argument("--mode", choices=("scout", "promote", "bar"), default="promote")
    result.add_argument("--metric", choices=("execute", "total"), default="execute")
    result.add_argument("--packet", required=True, type=int)
    result.add_argument("--window", required=True, type=int)
    result.add_argument("--warmup", type=int, default=10)
    result.add_argument("--regression-guardrail-ms", type=float, default=1.0)
    result.add_argument("--task-batch", action="store_true")
    result.add_argument("--control-env", action="append", default=[])
    result.add_argument("--candidate-env", action="append", default=[])
    result.add_argument("--max-load-one", type=float, default=3.0)
    result.add_argument("--timeout-seconds", type=float, default=600.0)
    return result


def main() -> int:
    args = parser().parse_args()
    if args.packet < 0 or args.window <= 0 or args.warmup < 0:
        raise MeasurementError("packet/window/warmup values are out of range")
    if (
        args.max_load_one <= 0
        or args.timeout_seconds <= 0
        or args.regression_guardrail_ms <= 0
    ):
        raise MeasurementError("load and timeout limits must be positive")
    if args.output_dir.exists():
        raise MeasurementError("output directory must not already exist")
    args.output_dir.mkdir(mode=0o700, parents=True)
    os.chmod(args.output_dir, 0o700)

    lanes = {
        "control": (args.control_bin, parse_assignments(args.control_env)),
        "candidate": (args.candidate_bin, parse_assignments(args.candidate_env)),
    }
    binaries = {
        lane: stable_file_receipt(binary, executable=True)
        for lane, (binary, _) in lanes.items()
    }
    inputs = {
        "streams": stream_set_receipt(args.streams),
        "rdram": stable_file_receipt(args.rdram),
    }
    preflight = quiet_machine(args.max_load_one)
    legs: list[dict[str, object]] = []
    expected_identity: dict[str, str] | None = None
    timing_schedule = [
        ("control", 1, 1),
        ("candidate", 1, 2),
        ("control", 2, 1),
        ("candidate", 2, 2),
        ("candidate", 3, 1),
        ("control", 3, 2),
        ("candidate", 4, 1),
        ("control", 4, 2),
        ("control", 5, 1),
        ("candidate", 5, 2),
        ("candidate", 6, 1),
        ("control", 6, 2),
    ]
    schedule = timing_schedule[:4] if args.mode == "scout" else timing_schedule
    if args.mode != "scout":
        schedule += [("candidate", pair, 1) for pair in range(7, 11)]

    for leg_index, (lane, pair, ordinal) in enumerate(schedule, 1):
        phase = "timing" if pair <= 6 else "identity"
        quiet_machine(args.max_load_one)
        binary, lane_env = lanes[lane]
        if stable_file_receipt(binary, executable=True) != binaries[lane]:
            raise MeasurementError("binary changed after preflight")
        env = {key: value for key, value in os.environ.items() if not key.startswith("FN64_")}
        env.update(lane_env)
        env.update(
            {
                "FN64_RAW_DPC_REPLAY_PACKET": str(args.packet),
                "FN64_RAW_DPC_REPLAY_WINDOW": str(args.window),
                "FN64_RAW_DPC_REPLAY_TASK_BATCH": "1" if args.task_batch else "0",
                "FN64_RAW_DPC_REPLAY_WARMUP": str(args.warmup),
                "FN64_RAW_DPC_REPLAY_REPEAT": "1",
            }
        )
        started = time.monotonic()
        process = subprocess.run(
            [str(binary), str(args.streams), str(args.rdram)],
            env=env,
            capture_output=True,
            text=True,
            timeout=args.timeout_seconds,
        )
        elapsed = time.monotonic() - started
        log = args.output_dir / f"leg-{leg_index:02d}-{phase}-{lane}.log"
        log.write_text(process.stdout + process.stderr)
        if process.returncode:
            raise MeasurementError(f"{lane} replay exited {process.returncode}; inspect private leg log")
        identity, metrics = parse_output(process.stdout)
        if expected_identity is None:
            expected_identity = identity
        elif identity != expected_identity:
            write_summary(
                args.output_dir,
                {
                    "schema": SCHEMA,
                    "status": "identity_mismatch",
                    "configuration": config_summary(args),
                    "preflight": preflight,
                    "binaries": binaries,
                    "inputs": inputs,
                    "lane_environment_sha256": {
                        name: environment_digest(values)
                        for name, (_, values) in lanes.items()
                    },
                    "expected_identity": expected_identity,
                    "observed_identity": identity,
                    "completed_legs": legs,
                    "failed_leg": {
                        "pair": pair,
                        "order": ordinal,
                        "phase": phase,
                        "lane": lane,
                    },
                },
            )
            raise MeasurementError("replay identity mismatch across fresh processes")
        if stable_file_receipt(binary, executable=True) != binaries[lane]:
            raise MeasurementError("binary changed during measurement")
        legs.append(
            {
                "pair": pair,
                "order": ordinal,
                "phase": phase,
                "lane": lane,
                "wall_seconds": round(elapsed, 6),
                "metrics": {name: metrics[name] for name in ("execute", "total")},
            }
        )

        if args.mode != "bar" and leg_index == 3:
            candidate_ms = float(legs[1]["metrics"][args.metric]["mean_ms"])
            controls_ms = [
                float(legs[index]["metrics"][args.metric]["mean_ms"])
                for index in (0, 2)
            ]
            guardrail = args.regression_guardrail_ms
            if all(candidate_ms >= control_ms + guardrail for control_ms in controls_ms):
                write_summary(
                    args.output_dir,
                    {
                        "schema": SCHEMA,
                        "status": "obvious_regression",
                        "configuration": config_summary(args),
                        "preflight": preflight,
                        "binaries": binaries,
                        "inputs": inputs,
                        "lane_environment_sha256": {
                            name: environment_digest(values)
                            for name, (_, values) in lanes.items()
                        },
                        "identity": expected_identity,
                        "legs": legs,
                        "regression_decision": {
                            "metric": args.metric,
                            "guardrail_ms": guardrail,
                            "candidate_ms": candidate_ms,
                            "bracketing_control_ms": controls_ms,
                        },
                    },
                )
                return 0

    status = {
        "scout": "scout_complete",
        "promote": "promoted_complete",
        "bar": "bar_complete",
    }[args.mode]
    document = {
        "schema": SCHEMA,
        "status": status,
        "configuration": config_summary(args),
        "preflight": preflight,
        "binaries": binaries,
        "inputs": inputs,
        "lane_environment_sha256": {
            lane: environment_digest(values) for lane, (_, values) in lanes.items()
        },
        "identity": expected_identity,
        "legs": legs,
        "comparison": comparison(legs, args.metric),
    }
    write_summary(args.output_dir, document)
    return 0


def config_summary(args: argparse.Namespace) -> dict[str, object]:
    timing_runs = 2 if args.mode == "scout" else 6
    identity_candidate_runs = 0 if args.mode == "scout" else 4
    return {
        "mode": args.mode,
        "metric": args.metric,
        "packet": args.packet,
        "window": args.window,
        "task_batch": args.task_batch,
        "warmup": args.warmup,
        "repeat": 1,
        "regression_guardrail_ms": args.regression_guardrail_ms,
        "timing_control_runs": timing_runs,
        "timing_candidate_runs": timing_runs,
        "identity_candidate_runs": identity_candidate_runs,
    }


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (MeasurementError, OSError, subprocess.SubprocessError) as error:
        print(f"benchmark-raw-dpc-replay: {error}", file=sys.stderr)
        raise SystemExit(1)
