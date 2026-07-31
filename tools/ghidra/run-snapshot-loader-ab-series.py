#!/usr/bin/env python3
"""Run the guarded snapshot loader A/B diagnostic ten consecutive times."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
from typing import Any


RUN_COUNT = 10
SHA256 = re.compile(r"[0-9a-f]{64}")
TOP_LEVEL_FIELDS = {
    "schema",
    "schema_version",
    "candidate_only",
    "role",
    "context",
    "program_snapshot_sha256",
    "input",
    "n64loaderwv",
    "tool_identity_sha256",
    "artifact_sha256",
    "resource_evidence_sha256",
    "completed_lanes",
    "production_ingest_performed",
}
ARTIFACT_FIELDS = {
    "evidence",
    "binary_config",
    "n64_config",
    "binary_pre",
    "binary_post",
    "n64_pre",
    "n64_post",
    "n64loaderwv_install_verification",
    "n64loaderwv_runtime_verification",
    "comparison",
}
RESOURCE_FIELDS = {
    "distribution_scan",
    "binary",
    "n64",
    "comparison",
    "distribution_verify",
}


class SeriesError(RuntimeError):
    pass


def fail(message: str) -> None:
    raise SeriesError(message)


def canonical_json(value: Any) -> bytes:
    return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        while chunk := stream.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def require_sha_map(value: Any, fields: set[str], label: str) -> dict[str, str]:
    if not isinstance(value, dict) or set(value) != fields:
        fail(f"{label} has the wrong fields")
    if any(not isinstance(item, str) or SHA256.fullmatch(item) is None for item in value.values()):
        fail(f"{label} contains an invalid SHA-256 digest")
    return value


def read_receipt(path: Path) -> tuple[dict[str, Any], dict[str, Any]]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        fail(f"could not read attempt receipt: {error}")
    if not isinstance(value, dict) or set(value) != TOP_LEVEL_FIELDS:
        fail("attempt receipt has the wrong fields")
    if value["schema"] != "fn64.ghidra-snapshot-loader-ab-receipt" or value["schema_version"] != 1:
        fail("attempt receipt has the wrong schema")
    if value["candidate_only"] is not True or value["production_ingest_performed"] is not False:
        fail("attempt receipt crossed the candidate-only authority boundary")
    if value["role"] != "differential_comparison" or value["context"] != "synthetic_zero_fill":
        fail("attempt receipt has the wrong diagnostic role or context")
    if value["completed_lanes"] != ["binary-loader", "n64loaderwv"]:
        fail("attempt receipt did not complete both lanes in order")
    if not isinstance(value["program_snapshot_sha256"], str) or SHA256.fullmatch(value["program_snapshot_sha256"]) is None:
        fail("attempt receipt has an invalid program snapshot digest")
    require_sha_map(value["artifact_sha256"], ARTIFACT_FIELDS, "attempt artifact identity")
    require_sha_map(value["resource_evidence_sha256"], RESOURCE_FIELDS, "attempt resource evidence")
    if not isinstance(value["input"], dict) or not isinstance(value["n64loaderwv"], dict):
        fail("attempt receipt is missing structured input or loader identity")
    if not isinstance(value["tool_identity_sha256"], dict) or not value["tool_identity_sha256"]:
        fail("attempt receipt is missing tool identity")
    if any(not isinstance(item, str) or SHA256.fullmatch(item) is None for item in value["tool_identity_sha256"].values()):
        fail("attempt tool identity contains an invalid SHA-256 digest")
    semantic = dict(value)
    del semantic["resource_evidence_sha256"]
    return value, semantic


def require_canonical_private_directory(raw: str, repo: Path) -> Path:
    path = Path(raw)
    if not path.is_absolute() or os.path.realpath(path) != str(path):
        fail("FN64_GHIDRA_LOADER_AB_SERIES_WORK must be absolute and canonical")
    if not path.is_dir() or path.is_symlink():
        fail("FN64_GHIDRA_LOADER_AB_SERIES_WORK must be a non-symlink directory")
    metadata = path.stat()
    if stat.S_IMODE(metadata.st_mode) != 0o700 or metadata.st_uid != os.getuid():
        fail("FN64_GHIDRA_LOADER_AB_SERIES_WORK must be caller-owned with mode 0700")
    if path == repo or repo in path.parents:
        fail("FN64_GHIDRA_LOADER_AB_SERIES_WORK must be outside the repository")
    if any(path.iterdir()):
        fail("FN64_GHIDRA_LOADER_AB_SERIES_WORK must be empty")
    return path


def parse_runner_output(path: Path, workspace: Path) -> tuple[Path, Path]:
    fields: dict[str, str] = {}
    lines = path.read_text(encoding="utf-8").splitlines()
    if lines.count("ghidra snapshot-loader-ab: complete") != 1:
        fail("attempt runner did not report completion exactly once")
    for line in lines:
        if "=" not in line:
            continue
        key, value = line.split("=", 1)
        if key not in {"attempt", "comparison", "receipt"} or key in fields:
            fail("attempt runner produced an unexpected or duplicate output field")
        fields[key] = value
    if set(fields) != {"attempt", "comparison", "receipt"}:
        fail("attempt runner did not report its three output paths")
    attempt = Path(fields["attempt"])
    if not attempt.is_absolute() or os.path.realpath(attempt) != str(attempt):
        fail("attempt runner reported a non-canonical attempt path")
    if attempt.parent != workspace or not attempt.is_dir() or attempt.is_symlink():
        fail("attempt runner reported an attempt outside the series workspace")
    comparison = Path(fields["comparison"])
    receipt = Path(fields["receipt"])
    if comparison != attempt / "out/comparison.json" or receipt != attempt / "out/receipt.json":
        fail("attempt runner reported unexpected artifact paths")
    for artifact in (comparison, receipt):
        if not artifact.is_file() or artifact.is_symlink():
            fail("attempt runner reported a missing or symlink artifact")
    return comparison, receipt


def exclusive_write(path: Path, value: bytes) -> None:
    with path.open("xb") as stream:
        stream.write(value)
    path.chmod(0o600)


def run(argv: list[str]) -> None:
    if len(argv) != 6:
        fail(
            "usage: tools/ghidra/run-snapshot-loader-ab-series.py "
            "PROGRAM_SNAPSHOT BANK MATERIALIZED_BANK EXTENSION_ZIP CONFORMANCE_RECEIPT"
        )
    repo = Path(__file__).resolve().parents[2]
    runner = repo / "tools/ghidra/run-snapshot-loader-ab.sh"
    if not runner.is_file() or runner.is_symlink() or not os.access(runner, os.X_OK):
        fail("canonical snapshot loader A/B runner is unavailable")
    series_raw = os.environ.get("FN64_GHIDRA_LOADER_AB_SERIES_WORK", "")
    if not series_raw:
        fail("FN64_GHIDRA_LOADER_AB_SERIES_WORK is required")
    series = require_canonical_private_directory(series_raw, repo)
    runs = series / "runs"
    workspace = series / "workspace"
    runs.mkdir(mode=0o700)
    workspace.mkdir(mode=0o700)

    baseline_bytes: bytes | None = None
    baseline: dict[str, Any] | None = None
    records: list[dict[str, Any]] = []
    for index in range(1, RUN_COUNT + 1):
        stdout = runs / f"run-{index:03d}.stdout.log"
        stderr = runs / f"run-{index:03d}.stderr.log"
        print(f"ghidra snapshot-loader-ab series: run {index}/{RUN_COUNT}", file=sys.stderr, flush=True)
        with stdout.open("xb") as stdout_stream, stderr.open("xb") as stderr_stream:
            completed = subprocess.run(
                [str(runner), argv[1], argv[2], argv[3], str(workspace), argv[4], argv[5]],
                stdin=subprocess.DEVNULL,
                stdout=stdout_stream,
                stderr=stderr_stream,
                check=False,
            )
        stdout.chmod(0o600)
        stderr.chmod(0o600)
        if completed.returncode != 0:
            fail(f"run {index}/{RUN_COUNT} failed; diagnostics retained in runs/run-{index:03d}.*.log")
        comparison, receipt_path = parse_runner_output(stdout, workspace)
        receipt, semantic = read_receipt(receipt_path)
        comparison_sha = sha256_file(comparison)
        if comparison_sha != receipt["artifact_sha256"]["comparison"]:
            fail(f"run {index}/{RUN_COUNT} comparison digest does not match its receipt")
        semantic_bytes = canonical_json(semantic)
        if baseline_bytes is None:
            baseline_bytes = semantic_bytes
            baseline = semantic
        elif semantic_bytes != baseline_bytes:
            fail(f"run {index}/{RUN_COUNT} semantic receipt or artifact identity drifted")
        records.append(
            {
                "run": index,
                "receipt_sha256": sha256_file(receipt_path),
                "resource_evidence_sha256": receipt["resource_evidence_sha256"],
            }
        )

    assert baseline_bytes is not None and baseline is not None
    semantic_path = series / "semantic-receipt.json"
    attempts_path = series / "attempt-receipts.jsonl"
    series_path = series / "receipt.json"
    exclusive_write(semantic_path, baseline_bytes)
    attempts_bytes = b"".join(canonical_json(record) for record in records)
    exclusive_write(attempts_path, attempts_bytes)
    series_receipt = {
        "schema": "fn64.ghidra-snapshot-loader-ab-series",
        "schema_version": 1,
        "run_count": RUN_COUNT,
        "required_clean_runs": RUN_COUNT,
        "candidate_only": True,
        "production_ingest_performed": False,
        "semantic_receipt_sha256": sha256_file(semantic_path),
        "attempt_receipts_sha256": sha256_file(attempts_path),
        "artifact_sha256": baseline["artifact_sha256"],
    }
    exclusive_write(series_path, canonical_json(series_receipt))
    print(
        f"ghidra snapshot-loader-ab series: {RUN_COUNT}/{RUN_COUNT} "
        f"semantic-and-artifact-identical clean runs receipt_sha256={sha256_file(series_path)}"
    )


if __name__ == "__main__":
    try:
        run(sys.argv)
    except SeriesError as error:
        print(f"ghidra snapshot-loader-ab series: {error}", file=sys.stderr)
        raise SystemExit(2)
