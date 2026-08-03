#!/usr/bin/env python3
"""Run candidate-only unseeded Ghidra discovery over one snapshot workspace."""

from __future__ import annotations

import fcntl
import hashlib
import json
import os
import re
import selectors
import signal
import stat
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import NoReturn

MIB = 1024 * 1024
GIB = 1024 * MIB
MAX_MANIFEST_BYTES = 16 * MIB
MAX_BANKS = 4096
MAX_BANK_BYTES = 128 * MIB
MAX_AGGREGATE_BANK_BYTES = 256 * MIB
MAX_SNAPSHOT_BYTES = 128 * MIB
MAX_AGGREGATE_SNAPSHOT_BYTES = GIB
MAX_LOG_BYTES = 16 * MIB
MAX_ATTEMPT_BYTES = 512 * MIB
ATTEMPT_RESULT_RESERVE_BYTES = MIB
MIN_FREE_DISK_BYTES = 2 * GIB
BUFFER_BYTES = MIB
HEX64 = re.compile(r"[0-9a-f]{64}")
ATTEMPT_NAME = re.compile(r"ghidra-snapshot-bank\.[A-Za-z0-9]+")


class QueueError(Exception):
    pass


@dataclass(frozen=True)
class QueueLimits:
    max_launches: int = 64
    max_wall_seconds: int = 6 * 60 * 60
    max_attempts_per_bank: int = 3
    max_ordinary_failures: int = 8
    max_log_bytes: int = MAX_LOG_BYTES
    max_attempt_bytes: int = MAX_ATTEMPT_BYTES
    min_free_disk_bytes: int = MIN_FREE_DISK_BYTES
    termination_grace_seconds: int = 30


@dataclass(frozen=True)
class Bank:
    index: int
    name: str
    bank_artifact: str
    bank_length: int
    bank_sha256: str
    snapshot_artifact: str
    snapshot_length: int
    snapshot_sha256: str
    program_snapshot_sha256: str
    seed_mode: str
    base_seed: int | None
    rom_space: str
    rom_start: int
    rom_end: int
    va_start: int
    va_end: int
    normalized_rom_sha256: str


@dataclass(frozen=True)
class RunnerSuccess:
    attempt_name: str
    receipt_sha256: str
    claims_sha256: str
    distribution_sha256: str
    unseeded_tool_sha256: str
    common_cohort_sha256: str
    distribution_file_count: int


def fail(message: str) -> NoReturn:
    raise QueueError(message)


def strict_object(pairs):
    value = {}
    for key, item in pairs:
        if key in value:
            fail(f"duplicate JSON field {key!r}")
        value[key] = item
    return value


def exact_fields(value: object, expected: set[str], label: str) -> dict:
    if not isinstance(value, dict) or set(value) != expected:
        fail(f"{label} has wrong fields")
    return value


def integer(value: object, label: str, maximum: int | None = None) -> int:
    if type(value) is not int or value < 0 or (maximum is not None and value > maximum):
        fail(f"{label} is not a bounded nonnegative integer")
    return value


def digest(value: object, label: str) -> str:
    if not isinstance(value, str) or HEX64.fullmatch(value) is None:
        fail(f"{label} is not canonical SHA-256")
    return value


def canonical_private_directory(path: Path, label: str) -> Path:
    if not path.is_absolute():
        fail(f"{label} must be absolute")
    try:
        resolved = path.resolve(strict=True)
        metadata = path.lstat()
    except OSError as error:
        fail(f"cannot inspect {label}: {error}")
    if resolved != path or not stat.S_ISDIR(metadata.st_mode) or stat.S_ISLNK(metadata.st_mode):
        fail(f"{label} must be a canonical non-symlink directory")
    if stat.S_IMODE(metadata.st_mode) != 0o700 or metadata.st_uid != os.getuid():
        fail(f"{label} must be caller-owned with mode 0700")
    for ancestor in (path, *path.parents):
        if (ancestor / ".git").exists():
            fail(f"{label} must be outside a Git worktree")
    return path


def read_json(path: Path, limit: int, label: str, *, private: bool = False) -> tuple[bytes, object]:
    data, _, _ = hash_regular(path, limit, label, retain=True, private=private)
    try:
        return data, json.loads(data, object_pairs_hook=strict_object)
    except (json.JSONDecodeError, UnicodeDecodeError) as error:
        fail(f"cannot parse {label}: {error}")


def hash_regular(
    path: Path,
    limit: int,
    label: str,
    *,
    retain: bool = False,
    program_wire: bool = False,
    private: bool = False,
) -> tuple[bytes, int, str] | tuple[None, int, str, str]:
    try:
        before = path.lstat()
    except OSError as error:
        fail(f"cannot inspect {label}: {error}")
    if not stat.S_ISREG(before.st_mode) or stat.S_ISLNK(before.st_mode) or before.st_size > limit:
        fail(f"{label} must be a bounded regular non-symlink file")
    if private and (before.st_uid != os.getuid() or stat.S_IMODE(before.st_mode) != 0o600):
        fail(f"{label} must be caller-owned with mode 0600")
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(path, flags)
    except OSError as error:
        fail(f"cannot open {label}: {error}")
    artifact = hashlib.sha256()
    semantic = hashlib.sha256(b"fn64.program-snapshot.v3\0") if program_wire else None
    chunks = []
    measured = 0
    tail = b""
    last_two = b""
    try:
        opened = os.fstat(descriptor)
        if not stat.S_ISREG(opened.st_mode) or (opened.st_dev, opened.st_ino) != (
            before.st_dev,
            before.st_ino,
        ):
            fail(f"{label} changed while opening")
        while True:
            block = os.read(descriptor, BUFFER_BYTES)
            if not block:
                break
            measured += len(block)
            if measured > limit:
                fail(f"{label} exceeds {limit} bytes")
            artifact.update(block)
            last_two = (last_two + block)[-2:]
            if retain:
                chunks.append(block)
            if semantic is not None:
                combined = tail + block
                if len(combined) > 1:
                    semantic.update(combined[:-1])
                tail = combined[-1:]
        after = os.fstat(descriptor)
    finally:
        os.close(descriptor)
    fields = ("st_dev", "st_ino", "st_size", "st_mtime_ns", "st_ctime_ns")
    if measured != before.st_size or any(
        getattr(before, field) != getattr(after, field) for field in fields
    ):
        fail(f"{label} changed while hashing")
    if semantic is not None:
        if tail != b"\n" or last_two in {b"\n\n", b"\r\n"}:
            fail(f"{label} must end in exactly one LF")
        return None, measured, artifact.hexdigest(), semantic.hexdigest()
    return b"".join(chunks), measured, artifact.hexdigest()


def validate_manifest(workspace: Path) -> tuple[bytes, str, dict, list[Bank]]:
    manifest_path = workspace / "snapshot-workspace.json"
    manifest_bytes, value = read_json(
        manifest_path, MAX_MANIFEST_BYTES, "snapshot manifest", private=True
    )
    top = exact_fields(
        value,
        {
            "schema", "schema_version", "state", "open_reason", "normalized_rom_sha256",
            "discovery", "limits", "snapshot_wire", "aggregate_snapshot_artifact_bytes",
            "rom_recompilation_complete", "remaining_recompilation_frontier", "intended_use",
            "banks",
        },
        "snapshot manifest",
    )
    if top["schema"] != "fn64.snapshot-workspace" or type(top["schema_version"]) is not int or top["schema_version"] != 4:
        fail("unsupported snapshot-workspace schema")
    rom_sha = digest(top["normalized_rom_sha256"], "normalized ROM digest")
    if top["intended_use"] != "candidate_ghidra_only":
        fail("snapshot workspace is not admitted for candidate-only Ghidra use")
    if top["rom_recompilation_complete"] is not False or top["remaining_recompilation_frontier"] != "proven_bank_and_callable_owner_closure":
        fail("snapshot workspace overstates recompilation completion")

    discovery = exact_fields(top["discovery"], {"selected", "outcomes"}, "discovery")
    strategies = {"boot_bank_open", "boot_bank_only", "recovered_vrom", "recovered_overlays", "untabled_delta_vote"}
    if discovery["selected"] not in strategies or not isinstance(discovery["outcomes"], list) or not discovery["outcomes"]:
        fail("invalid discovery receipt")
    seen_strategies = set()
    for outcome in discovery["outcomes"]:
        outcome = exact_fields(
            outcome,
            {
                "strategy", "candidate_tables", "admitted_tables", "admitted_intervals",
                "proven_mappings", "supported_mappings", "decoded_file_limit_hits",
                "request_dma_open_rows", "request_dma_incomplete",
                "request_dma_input_limit_hit", "physical_wrapper_candidates_examined",
                "wrapper_semantic_proof_unavailable",
                "physical_wrapper_candidate_limit_hit",
            },
            "strategy outcome",
        )
        if outcome["strategy"] not in strategies or outcome["strategy"] in seen_strategies:
            fail("invalid or duplicate discovery strategy")
        seen_strategies.add(outcome["strategy"])
        boolean_fields = {
            "request_dma_incomplete", "request_dma_input_limit_hit",
            "physical_wrapper_candidate_limit_hit",
        }
        if any(type(outcome[field]) is not bool for field in boolean_fields):
            fail("strategy outcome has invalid boolean frontier")
        for field in set(outcome) - {"strategy"} - boolean_fields:
            integer(outcome[field], f"strategy outcome {field}")
    if discovery["selected"] not in seen_strategies:
        fail("selected discovery strategy has no outcome")

    expected_limits = {
        "max_rom_bytes": 64 * MIB,
        "max_banks": MAX_BANKS,
        "max_snapshot_artifact_bytes": MAX_SNAPSHOT_BYTES,
        "max_aggregate_snapshot_artifact_bytes": MAX_AGGREGATE_SNAPSHOT_BYTES,
        "max_discovery_decoded_vrom_file_bytes": 64 * MIB,
        "max_preparation_decoded_vrom_file_bytes": 64 * MIB,
        "max_projected_fact_rows": 4_000_000,
        "max_projected_fact_bytes": 256 * MIB,
        "max_aggregate_materialized_bytes": 256 * MIB,
        "max_cross_bank_authority_records": 1_048_576,
    }
    limits = exact_fields(top["limits"], set(expected_limits), "producer limits")
    if limits != expected_limits:
        fail("snapshot workspace producer limits do not match v4")
    wire = exact_fields(top["snapshot_wire"], {"schema_version", "authority", "duplicates_fact_db_per_bank", "remaining_large_rom_frontier"}, "snapshot wire")
    if type(wire["schema_version"]) is not int or wire != {"schema_version": 6, "authority": "diagnostic_only", "duplicates_fact_db_per_bank": False, "remaining_large_rom_frontier": "streaming_v6"}:
        fail("snapshot wire is not admitted projected diagnostic v6")

    raw_banks = top["banks"]
    if not isinstance(raw_banks, list) or len(raw_banks) > MAX_BANKS:
        fail("snapshot workspace has invalid bank count")
    state = top["state"]
    aggregate_declared = integer(top["aggregate_snapshot_artifact_bytes"], "aggregate snapshot bytes", MAX_AGGREGATE_SNAPSHOT_BYTES)
    if state == "open":
        if top["open_reason"] != "no_proven_banks" or raw_banks or aggregate_declared != 0:
            fail("invalid open snapshot workspace")
        if {entry.name for entry in workspace.iterdir()} != {"snapshot-workspace.json"}:
            fail("open snapshot workspace contains unmanifested artifacts")
        return manifest_bytes, hashlib.sha256(manifest_bytes).hexdigest(), top, []
    if state != "composed" or top["open_reason"] is not None or not raw_banks:
        fail("invalid composed snapshot workspace")

    banks = []
    aggregate_bank = 0
    aggregate_snapshot = 0
    previous_name = None
    expected_reserved = {"snapshot-workspace.json"}
    for index, raw in enumerate(raw_banks):
        bank = exact_fields(raw, {"index", "bank", "backing", "va_start", "va_end", "byte_length", "backing_evidence_fact_indices", "bank_sha256", "bank_artifact", "snapshot_artifact", "snapshot_artifact_byte_length", "snapshot_artifact_sha256", "program_snapshot_sha256", "ghidra_seeds"}, f"bank {index}")
        if integer(bank["index"], f"bank {index} index") != index:
            fail("bank indices must be contiguous")
        name = bank["bank"]
        if not isinstance(name, str) or not name or len(name.encode()) > 4096 or any(ord(char) < 32 for char in name):
            fail(f"bank {index} has invalid name")
        if previous_name is not None and previous_name >= name:
            fail("bank names must be strictly sorted and unique")
        previous_name = name
        if not isinstance(bank["backing"], dict):
            fail(f"bank {index} has invalid backing")
        backing_kind = bank["backing"].get("kind")
        if backing_kind == "materialized":
            exact_fields(
                bank["backing"],
                {"kind", "receipt_sha256", "output_start", "output_end"},
                f"bank {index} materialized backing",
            )
            fail(f"bank {index} materialized backing is unsupported by the affine-only Ghidra workspace runner")
        backing = exact_fields(
            bank["backing"],
            {"kind", "rom_space", "rom_start", "rom_end"},
            f"bank {index} affine backing",
        )
        if backing["kind"] != "rom_affine" or backing["rom_space"] not in {"Physical", "Virtual"}:
            fail(f"bank {index} has invalid affine ROM backing")
        rom_space = backing["rom_space"]
        rom_start = integer(backing["rom_start"], f"bank {index} rom_start", 0xFFFF_FFFF)
        rom_end = integer(backing["rom_end"], f"bank {index} rom_end", 0xFFFF_FFFF)
        va_start = integer(bank["va_start"], f"bank {index} va_start", 0xFFFF_FFFF)
        va_end = integer(bank["va_end"], f"bank {index} va_end", 0xFFFF_FFFF)
        length = integer(bank["byte_length"], f"bank {index} byte_length", MAX_BANK_BYTES)
        if length == 0 or rom_end - rom_start != length or va_end - va_start != length or va_start % 4 or length % 4:
            fail(f"bank {index} has inconsistent or unaligned geometry")
        evidence = bank["backing_evidence_fact_indices"]
        if not isinstance(evidence, list) or any(type(item) is not int or item < 0 for item in evidence) or evidence != sorted(set(evidence)):
            fail(f"bank {index} has invalid backing evidence")
        if (rom_space == "Physical" and evidence) or (rom_space == "Virtual" and len(evidence) != 1):
            fail(f"bank {index} backing evidence does not match ROM space")
        bank_artifact = f"bank-{index:06}.bin"
        snapshot_artifact = f"bank-{index:06}.snapshot.json"
        if bank["bank_artifact"] != bank_artifact or bank["snapshot_artifact"] != snapshot_artifact:
            fail(f"bank {index} does not use fixed artifact names")
        snapshot_length = integer(bank["snapshot_artifact_byte_length"], f"bank {index} snapshot length", MAX_SNAPSHOT_BYTES)
        bank_sha = digest(bank["bank_sha256"], f"bank {index} artifact digest")
        snapshot_sha = digest(bank["snapshot_artifact_sha256"], f"bank {index} snapshot artifact digest")
        program_sha = digest(bank["program_snapshot_sha256"], f"bank {index} program snapshot digest")
        _, actual_bank_length, actual_bank_sha = hash_regular(
            workspace / bank_artifact, MAX_BANK_BYTES, f"bank {index} artifact", private=True
        )
        _, actual_snapshot_length, actual_snapshot_sha, actual_program_sha = hash_regular(
            workspace / snapshot_artifact,
            MAX_SNAPSHOT_BYTES,
            f"bank {index} snapshot",
            program_wire=True,
            private=True,
        )
        if (length, bank_sha) != (actual_bank_length, actual_bank_sha) or (snapshot_length, snapshot_sha, program_sha) != (actual_snapshot_length, actual_snapshot_sha, actual_program_sha):
            fail(f"bank {index} artifact identity mismatch")
        seeds = bank["ghidra_seeds"]
        if not isinstance(seeds, dict):
            fail(f"bank {index} has invalid Ghidra seeds")
        mode = seeds.get("mode")
        base_seed = None
        if mode == "discovery_only":
            exact_fields(seeds, {"mode", "role"}, f"bank {index} seeds")
            if seeds["role"] != "candidate_only":
                fail(f"bank {index} discovery-only role is not candidate-only")
        elif mode == "base_only":
            exact_fields(seeds, {"mode", "base_seed", "base_seed_role"}, f"bank {index} seeds")
            if seeds["base_seed_role"] != "proven_owner":
                fail(f"bank {index} base seed is not proven-owner evidence")
            base_seed = integer(seeds["base_seed"], f"bank {index} base seed", 0xFFFF_FFFF)
        elif mode == "paired":
            exact_fields(seeds, {"mode", "base_seed", "base_seed_role", "snapshot_seed", "snapshot_seed_role", "snapshot_seed_assessment"}, f"bank {index} seeds")
            if seeds["base_seed_role"] != "proven_owner" or seeds["snapshot_seed_role"] != "assessed_owner" or seeds["snapshot_seed_assessment"] not in {"proven", "candidate", "ambiguous"}:
                fail(f"bank {index} seed roles are not admitted")
            base_seed = integer(seeds["base_seed"], f"bank {index} base seed", 0xFFFF_FFFF)
            snapshot_seed = integer(seeds["snapshot_seed"], f"bank {index} snapshot seed", 0xFFFF_FFFF)
            if snapshot_seed == base_seed or snapshot_seed % 4 or not va_start <= snapshot_seed < va_end:
                fail(f"bank {index} snapshot seed is invalid")
        else:
            fail(f"bank {index} has unknown seed mode")
        if base_seed is not None and (base_seed % 4 or not va_start <= base_seed < va_end):
            fail(f"bank {index} base seed is invalid")
        aggregate_bank += length
        aggregate_snapshot += snapshot_length
        expected_reserved.update({bank_artifact, snapshot_artifact})
        banks.append(Bank(index, name, bank_artifact, length, bank_sha, snapshot_artifact, snapshot_length, snapshot_sha, program_sha, mode, base_seed, rom_space, rom_start, rom_end, va_start, va_end, rom_sha))
    if aggregate_bank > MAX_AGGREGATE_BANK_BYTES or aggregate_snapshot != aggregate_declared:
        fail("snapshot workspace aggregate byte count is invalid")
    for entry in workspace.iterdir():
        if entry.name not in expected_reserved:
            fail(f"unmanifested snapshot-workspace artifact {entry.name}")
    return manifest_bytes, hashlib.sha256(manifest_bytes).hexdigest(), top, banks


def publish_new(path: Path, value: object) -> bytes:
    encoded = (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()
    temporary = path.parent / f".{path.name}.tmp-{os.getpid()}"
    descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        with os.fdopen(descriptor, "wb", closefd=True) as stream:
            stream.write(encoded)
            stream.flush()
            os.fsync(stream.fileno())
        os.link(temporary, path)
        directory = os.open(path.parent, os.O_RDONLY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        temporary.unlink(missing_ok=True)
    return encoded


def file_identity(path: Path, label: str) -> dict:
    _, length, sha = hash_regular(path, 64 * MIB, label)
    return {"byte_length": length, "sha256": sha}


def acquire_lock(path: Path) -> int:
    flags = os.O_RDWR | os.O_CREAT | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(path, flags, 0o600)
    metadata = os.fstat(descriptor)
    if not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.getuid() or stat.S_IMODE(metadata.st_mode) != 0o600:
        os.close(descriptor)
        fail("queue lock must be a caller-owned mode-0600 regular file")
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        os.close(descriptor)
        fail("another Ghidra snapshot-workspace queue is active")
    os.set_inheritable(descriptor, True)
    return descriptor


def ensure_private_subdirectory(path: Path, label: str) -> None:
    try:
        path.mkdir(mode=0o700)
    except FileExistsError:
        pass
    metadata = path.lstat()
    if not stat.S_ISDIR(metadata.st_mode) or stat.S_ISLNK(metadata.st_mode) or metadata.st_uid != os.getuid() or stat.S_IMODE(metadata.st_mode) != 0o700:
        fail(f"{label} must be a caller-owned mode-0700 non-symlink directory")


def ensure_request(
    output: Path,
    manifest_sha: str,
    rom_sha: str,
    queue_script: Path,
    runner: Path,
    stage: Path,
    ingest: Path,
    limits: QueueLimits,
) -> tuple[dict, str]:
    request = {
        "schema": "fn64.ghidra-snapshot-workspace-request",
        "schema_version": 1,
        "source_manifest_sha256": manifest_sha,
        "normalized_rom_sha256": rom_sha,
        "execution_mode": "candidate-only-sequential",
        "tools": {
            "queue": file_identity(queue_script, "snapshot-workspace queue"),
            "runner": file_identity(runner, "snapshot-bank runner"),
            "stage": file_identity(stage, "stage helper"),
            "ingest": file_identity(ingest, "ingest helper"),
        },
        "caps": {
            "max_launches": limits.max_launches,
            "max_wall_seconds": limits.max_wall_seconds,
            "max_attempts_per_bank": limits.max_attempts_per_bank,
            "max_ordinary_failures": limits.max_ordinary_failures,
            "max_log_bytes": limits.max_log_bytes,
            "max_attempt_bytes": limits.max_attempt_bytes,
            "min_free_disk_bytes": limits.min_free_disk_bytes,
            "termination_grace_seconds": limits.termination_grace_seconds,
        },
    }
    encoded = (json.dumps(request, sort_keys=True, separators=(",", ":")) + "\n").encode()
    path = output / "queue-request.json"
    if path.exists():
        existing, _ = read_json(path, MIB, "queue request", private=True)
        if existing != encoded:
            fail("queue request does not match this invocation")
    else:
        if any(output.iterdir()):
            fail("new queue workspace must be empty")
        publish_new(path, request)
    return request, hashlib.sha256(encoded).hexdigest()


def stable_bank_value(bank: Bank, request_sha: str, manifest_sha: str, attempt: int) -> dict:
    return {
        "queue_request_sha256": request_sha,
        "source_manifest_sha256": manifest_sha,
        "attempt": attempt,
        "bank": {
            "index": bank.index, "name": bank.name,
            "bank_sha256": bank.bank_sha256,
            "snapshot_artifact_sha256": bank.snapshot_sha256,
            "program_snapshot_sha256": bank.program_snapshot_sha256,
            "base_seed": bank.base_seed,
        },
    }


def validate_attempt_result(
    attempt_path: Path,
    bank: Bank,
    request_sha: str,
    manifest_sha: str,
) -> tuple[dict, RunnerSuccess | None, str]:
    attempt_number = int(attempt_path.name)
    result_bytes, result_value = read_json(
        attempt_path / "result.json", MIB, "attempt result", private=True
    )
    result = exact_fields(
        result_value,
        {
            "schema", "schema_version", "state", "failure_class", "runner_exit_status",
            "runner_attempt", "runner_receipt_sha256", "tool_claims_sha256",
            "ghidra_distribution_manifest_sha256", "unseeded_tool_manifest_sha256",
            "common_cohort_sha256", "stop_scheduling", "stdout", "stderr",
            "queue_request_sha256", "source_manifest_sha256", "attempt", "bank",
        },
        "attempt result",
    )
    if result["schema"] != "fn64.ghidra-snapshot-workspace-attempt" or type(result["schema_version"]) is not int or result["schema_version"] != 1:
        fail("attempt result has wrong schema")
    expected_stable = stable_bank_value(bank, request_sha, manifest_sha, attempt_number)
    integer(result["attempt"], "attempt number", 999999)
    for field in ("queue_request_sha256", "source_manifest_sha256", "attempt", "bank"):
        if result[field] != expected_stable[field]:
            fail(f"bank {bank.index} attempt identity mismatch")
    for stream_name in ("stdout", "stderr"):
        identity = result[stream_name]
        if identity is None:
            if result["failure_class"] != "abandoned_before_terminal_receipt":
                fail("terminal attempt result lacks log identity")
            continue
        identity = exact_fields(identity, {"byte_length", "sha256"}, f"{stream_name} identity")
        _, length, sha = hash_regular(
            attempt_path / f"{stream_name}.log", 64 * MIB, f"runner {stream_name}", private=True
        )
        if identity != {"byte_length": length, "sha256": sha}:
            fail(f"runner {stream_name} identity mismatch")
    success_fields = (
        "runner_attempt", "runner_receipt_sha256", "tool_claims_sha256",
        "ghidra_distribution_manifest_sha256", "unseeded_tool_manifest_sha256",
        "common_cohort_sha256",
    )
    runner_success = None
    if result["state"] == "success":
        if result["failure_class"] is not None or result["runner_exit_status"] != 0 or result["stop_scheduling"] is not False:
            fail("successful attempt has inconsistent terminal fields")
        if any(value is None for value in (result[field] for field in success_fields)):
            fail("successful attempt lacks runner identities")
        runner_success = validate_runner_success(attempt_path / "runner-workspace", bank)
        expected = {
            "runner_attempt": runner_success.attempt_name,
            "runner_receipt_sha256": runner_success.receipt_sha256,
            "tool_claims_sha256": runner_success.claims_sha256,
            "ghidra_distribution_manifest_sha256": runner_success.distribution_sha256,
            "unseeded_tool_manifest_sha256": runner_success.unseeded_tool_sha256,
            "common_cohort_sha256": runner_success.common_cohort_sha256,
        }
        if any(result[field] != value for field, value in expected.items()):
            fail("successful attempt does not bind its retained runner completion")
    elif result["state"] == "failure":
        allowed = {
            "abandoned_before_terminal_receipt", "invalid_runner_completion", "runner_exit",
            "queue_interrupted", "queue_wall_cap", "log_cap", "attempt_output_cap",
        }
        if result["failure_class"] not in allowed or any(result[field] is not None for field in success_fields):
            fail("failed attempt has inconsistent terminal fields")
        if result["runner_exit_status"] is not None and type(result["runner_exit_status"]) is not int:
            fail("failed attempt has invalid runner status")
        failure_class = result["failure_class"]
        status = result["runner_exit_status"]
        if failure_class == "abandoned_before_terminal_receipt" and status is not None:
            fail("abandoned attempt unexpectedly has a runner status")
        if failure_class == "invalid_runner_completion" and status != 0:
            fail("invalid runner completion does not have status zero")
        if failure_class == "runner_exit" and (type(status) is not int or status == 0):
            fail("runner-exit failure does not have a nonzero status")
        if failure_class not in {"abandoned_before_terminal_receipt", "invalid_runner_completion"} and type(status) is not int:
            fail("terminal runner failure lacks a status")
        expected_stop = result["failure_class"] in {
            "queue_interrupted", "queue_wall_cap", "log_cap", "attempt_output_cap"
        }
        if result["stop_scheduling"] is not expected_stop:
            fail("failed attempt has inconsistent stop-scheduling state")
    else:
        fail("attempt result has unknown state")
    return result, runner_success, hashlib.sha256(result_bytes).hexdigest()


def validate_runner_success(attempt_workspace: Path, bank: Bank) -> RunnerSuccess:
    workspace_metadata = attempt_workspace.lstat()
    if not stat.S_ISDIR(workspace_metadata.st_mode) or stat.S_ISLNK(workspace_metadata.st_mode) or workspace_metadata.st_uid != os.getuid() or stat.S_IMODE(workspace_metadata.st_mode) != 0o700:
        fail("runner workspace must be a caller-owned mode-0700 directory")
    entries = list(attempt_workspace.iterdir())
    candidates = [path for path in entries if ATTEMPT_NAME.fullmatch(path.name)]
    cache = attempt_workspace / ".fn64-ghidra-distribution-manifests"
    if len(candidates) != 1 or {path.name for path in entries} != {
        candidates[0].name,
        cache.name,
    }:
        fail("runner workspace does not contain exactly one attempt and its distribution cache")
    cache_metadata = cache.lstat()
    if not stat.S_ISDIR(cache_metadata.st_mode) or stat.S_ISLNK(cache_metadata.st_mode) or cache_metadata.st_uid != os.getuid() or stat.S_IMODE(cache_metadata.st_mode) != 0o700:
        fail("runner distribution cache must be a caller-owned mode-0700 directory")
    retained = candidates[0]
    retained_metadata = retained.lstat()
    if not stat.S_ISDIR(retained_metadata.st_mode) or stat.S_ISLNK(retained_metadata.st_mode) or retained_metadata.st_uid != os.getuid() or stat.S_IMODE(retained_metadata.st_mode) != 0o700:
        fail("retained runner attempt must be a caller-owned mode-0700 directory")
    receipt_path = retained / "out/receipt.json"
    receipt_bytes, receipt_value = read_json(receipt_path, MIB, "runner receipt", private=True)
    receipt = exact_fields(
        receipt_value,
        {
            "schema", "schema_version", "execution_mode", "paired_comparison_complete",
            "completed_modes", "program_snapshot_sha256", "bank", "seeds",
            "evidence_sha256", "request_sha256", "unseeded_tool_manifest_sha256",
            "tool_claims_sha256", "ghidra_distribution_manifest_complete",
            "ghidra_distribution_manifest_sha256", "ghidra_distribution_file_count",
            "tool_artifact_scope", "configuration_sha256", "provider_jsonl_sha256",
            "resource_evidence_sha256",
        },
        "runner receipt",
    )
    if receipt["schema"] != "fn64.ghidra-snapshot-bank-receipt" or type(receipt["schema_version"]) is not int or receipt["schema_version"] != 1:
        fail("runner receipt has wrong schema")
    expected_execution = "discovery-only" if bank.seed_mode == "discovery_only" else "unseeded-only"
    if receipt["execution_mode"] != expected_execution or receipt["paired_comparison_complete"] is not False or receipt["completed_modes"] != ["unseeded"]:
        fail("runner receipt is not the requested candidate-only execution")
    expected_seeds = (
        {"mode": "discovery_only", "role": "candidate_only"}
        if bank.seed_mode == "discovery_only" else
        {"mode": "base_only", "base_seed": bank.base_seed}
    )
    if receipt["program_snapshot_sha256"] != bank.program_snapshot_sha256 or receipt["bank"] != bank.name or receipt["seeds"] != expected_seeds:
        fail("runner receipt does not match queued bank")

    _, input_bank_length, input_bank_sha = hash_regular(
        retained / "inputs/bank.bin", MAX_BANK_BYTES, "retained bank input", private=True
    )
    _, input_snapshot_length, input_snapshot_sha, input_program_sha = hash_regular(
        retained / "inputs/program-snapshot.json",
        MAX_SNAPSHOT_BYTES,
        "retained program snapshot",
        program_wire=True,
        private=True,
    )
    if (input_bank_length, input_bank_sha) != (bank.bank_length, bank.bank_sha256) or (
        input_snapshot_length, input_snapshot_sha, input_program_sha
    ) != (bank.snapshot_length, bank.snapshot_sha256, bank.program_snapshot_sha256):
        fail("runner retained the wrong producer inputs")
    if receipt["ghidra_distribution_manifest_complete"] is not True:
        fail("runner did not complete its Ghidra distribution inventory")
    if receipt["tool_artifact_scope"] != "all-ghidra-install-regular-files,jdk-java,fn64-analysis-scripts,and-bound-orchestration-helpers":
        fail("runner receipt has unknown tool artifact scope")
    exact_fields(receipt["configuration_sha256"], {"unseeded"}, "configuration digests")
    exact_fields(receipt["provider_jsonl_sha256"], {"unseeded"}, "provider digests")
    resources = exact_fields(
        receipt["resource_evidence_sha256"],
        {
            "ghidra_distribution_scan_log", "ghidra_distribution_scan",
            "ghidra_distribution_verify_log", "ghidra_distribution_verify",
            "stage", "unseeded", "ingest",
        },
        "resource evidence digests",
    )

    bound_files = {
        "evidence_sha256": ("raw/evidence.json", 128 * MIB),
        "request_sha256": ("request.json", MIB),
        "unseeded_tool_manifest_sha256": ("tool-unseeded.json", MIB),
        "tool_claims_sha256": ("out/tool-claims.json", 128 * MIB),
        "ghidra_distribution_manifest_sha256": ("tool-artifacts/ghidra-distribution.json", 128 * MIB),
    }
    measured = {}
    measured_lengths = {}
    for field, (relative, limit) in bound_files.items():
        _, length, actual = hash_regular(
            retained / relative, limit, relative, private=True
        )
        if length == 0:
            fail(f"runner retained empty required artifact {relative}")
        if digest(receipt[field], field) != actual:
            fail(f"runner receipt does not bind {relative}")
        measured[field] = actual
        measured_lengths[field] = length

    _, evidence_value = read_json(
        retained / "raw/evidence.json", 128 * MIB, "runner evidence", private=True
    )
    evidence = exact_fields(
        evidence_value,
        {"schema", "schema_version", "program_snapshot_sha256", "input", "backing", "artifact", "seeds"},
        "runner evidence",
    )
    expected_evidence_seeds = expected_seeds
    evidence_input = exact_fields(
        evidence["input"],
        {"normalized_rom_sha256", "bank", "bank_bytes_sha256", "mapping_sha256", "va_start", "va_end"},
        "runner evidence input",
    )
    digest(evidence_input["mapping_sha256"], "mapping digest")
    if evidence["schema"] != "fn64.snapshot-bank-evidence" or type(evidence["schema_version"]) is not int or evidence["schema_version"] != (3 if bank.seed_mode == "discovery_only" else 2) or evidence["program_snapshot_sha256"] != bank.program_snapshot_sha256 or evidence["seeds"] != expected_evidence_seeds:
        fail("runner evidence has wrong schema or seeds")
    if evidence_input != {
        **evidence_input,
        "normalized_rom_sha256": bank.normalized_rom_sha256,
        "bank": bank.name,
        "bank_bytes_sha256": bank.bank_sha256,
        "va_start": bank.va_start,
        "va_end": bank.va_end,
    }:
        fail("runner evidence input does not match queued bank")
    backing = exact_fields(evidence["backing"], {"rom_space", "rom_start", "rom_end"}, "runner evidence backing")
    integer(backing["rom_start"], "evidence ROM start", 0xFFFF_FFFF)
    integer(backing["rom_end"], "evidence ROM end", 0xFFFF_FFFF)
    if backing != {"rom_space": bank.rom_space, "rom_start": bank.rom_start, "rom_end": bank.rom_end}:
        fail("runner evidence backing does not match queued bank")
    evidence_artifact = exact_fields(evidence["artifact"], {"byte_length", "sha256"}, "runner evidence artifact")
    integer(evidence_artifact["byte_length"], "evidence artifact length", MAX_BANK_BYTES)
    if evidence_artifact != {"byte_length": bank.bank_length, "sha256": bank.bank_sha256}:
        fail("runner evidence artifact does not match queued bank")
    direct = [
        (receipt["configuration_sha256"]["unseeded"], "config/unseeded.json"),
        (receipt["provider_jsonl_sha256"]["unseeded"], "modes/unseeded/out/provider.jsonl"),
        (resources["ghidra_distribution_scan_log"], "diagnostics/ghidra-distribution-scan.log"),
        (resources["ghidra_distribution_scan"], "diagnostics/ghidra-distribution-scan-memory.jsonl"),
        (resources["ghidra_distribution_verify_log"], "diagnostics/ghidra-distribution-verify.log"),
        (resources["ghidra_distribution_verify"], "diagnostics/ghidra-distribution-verify-memory.jsonl"),
        (resources["stage"], "diagnostics/stage-memory.jsonl"),
        (resources["unseeded"], "modes/unseeded/diagnostics/memory.jsonl"),
        (resources["ingest"], "diagnostics/ingest-memory.jsonl"),
    ]
    for expected, relative in direct:
        _, length, actual = hash_regular(retained / relative, 128 * MIB, relative, private=True)
        if length == 0:
            fail(f"runner retained empty required artifact {relative}")
        if digest(expected, relative) != actual:
            fail(f"runner receipt does not bind {relative}")

    _, distribution = read_json(
        retained / "tool-artifacts/ghidra-distribution.json",
        128 * MIB,
        "Ghidra distribution manifest",
        private=True,
    )
    distribution = exact_fields(
        distribution, {"schema", "schema_version", "files"}, "Ghidra distribution manifest"
    )
    if distribution["schema"] != "fn64.ghidra-distribution-manifest" or type(distribution["schema_version"]) is not int or distribution["schema_version"] != 1 or not isinstance(distribution["files"], list) or not distribution["files"]:
        fail("invalid Ghidra distribution manifest")
    if integer(receipt["ghidra_distribution_file_count"], "Ghidra distribution file count") != len(distribution["files"]):
        fail("Ghidra distribution file count mismatch")

    expected_cached_name = f'{measured["ghidra_distribution_manifest_sha256"]}.json'
    cached_entries = list(cache.iterdir())
    if [path.name for path in cached_entries] != [expected_cached_name]:
        fail("runner distribution cache does not contain exactly its content-addressed manifest")
    _, cached_length, cached_sha = hash_regular(
        cached_entries[0], MAX_MANIFEST_BYTES, "cached Ghidra distribution manifest", private=True
    )
    if (cached_length, cached_sha) != (
        measured_lengths["ghidra_distribution_manifest_sha256"],
        measured["ghidra_distribution_manifest_sha256"],
    ):
        fail("runner distribution cache does not match its retained manifest")

    _, tool_value = read_json(retained / "tool-unseeded.json", MIB, "unseeded tool manifest", private=True)
    tool = exact_fields(
        tool_value,
        {"schema", "schema_version", "tool_name", "tool_version", "artifacts"},
        "unseeded tool manifest",
    )
    if tool["schema"] != "fn64.tool-artifact-manifest" or type(tool["schema_version"]) is not int or tool["schema_version"] != 1 or tool["tool_name"] != "ghidra-headless-unseeded" or not isinstance(tool["tool_version"], str) or not tool["tool_version"]:
        fail("invalid unseeded tool manifest")
    expected_path_order = [
        "tool-artifacts/Fn64ExportCandidates.java",
        "tool-artifacts/analyzeHeadless",
        "tool-artifacts/application.properties",
        "tool-artifacts/ghidra-distribution.json",
        "tool-artifacts/java",
        "tool-artifacts/orchestration.json",
    ]
    if bank.seed_mode != "discovery_only":
        expected_path_order.insert(1, "tool-artifacts/Fn64SeedFunctions.java")
    expected_paths = set(expected_path_order)
    artifacts = tool["artifacts"]
    if not isinstance(artifacts, list) or len(artifacts) != len(expected_paths):
        fail("unseeded tool manifest has wrong artifact count")
    if [artifact.get("path") if isinstance(artifact, dict) else None for artifact in artifacts] != expected_path_order:
        fail("unseeded tool manifest artifact order is not canonical")
    seen_paths = set()
    artifact_hashes = {}
    for index, artifact_value in enumerate(artifacts):
        artifact = exact_fields(artifact_value, {"path", "byte_length", "sha256"}, f"tool artifact {index}")
        relative = artifact["path"]
        if relative not in expected_paths or relative in seen_paths:
            fail("unseeded tool manifest has an unexpected or duplicate path")
        seen_paths.add(relative)
        _, length, actual = hash_regular(retained / relative, 128 * MIB, relative, private=True)
        if length == 0:
            fail(f"runner retained empty tool artifact {relative}")
        if integer(artifact["byte_length"], f"{relative} length", 128 * MIB) != length or digest(artifact["sha256"], relative) != actual:
            fail(f"unseeded tool manifest does not bind {relative}")
        artifact_hashes[relative] = actual
    if seen_paths != expected_paths:
        fail("unseeded tool manifest is incomplete")

    _, request_value = read_json(retained / "request.json", MIB, "ingest request", private=True)
    request = exact_fields(request_value, {"schema", "schema_version", "runs"}, "ingest request")
    if request["schema"] != "fn64.tool-ingest-request" or type(request["schema_version"]) is not int or request["schema_version"] != 1 or not isinstance(request["runs"], list) or len(request["runs"]) != 1:
        fail("invalid ingest request")
    run = exact_fields(
        request["runs"][0],
        {"bank", "jsonl", "tool", "tool_artifact_manifest", "role", "lineage_artifacts"},
        "ingest run",
    )
    run_tool = exact_fields(run["tool"], {"name", "version", "build_sha256"}, "ingest run tool")
    expected_lineage = [
        {"role": "tool_configuration", "path": "config/unseeded.json"},
        {"role": "evidence_manifest", "path": "raw/evidence.json"},
    ]
    if run != {
        "bank": bank.name,
        "jsonl": "modes/unseeded/out/provider.jsonl",
        "tool": run_tool,
        "tool_artifact_manifest": "tool-unseeded.json",
        "role": "function_boundary_candidates",
        "lineage_artifacts": expected_lineage,
    } or run_tool != {
        "name": "ghidra-headless-unseeded",
        "version": tool["tool_version"],
        "build_sha256": measured["unseeded_tool_manifest_sha256"],
    }:
        fail("ingest request does not bind the retained unseeded run")

    _, config_value = read_json(
        retained / "config/unseeded.json", MIB, "unseeded configuration", private=True
    )
    config_fields = {
        "schema", "schema_version", "mode", "bank", "va_start", "va_end",
        "base_seed", "snapshot_seed", "loader", "processor", "cspec", "ghidra_version",
        "analysis_timeout_seconds", "max_cpu", "heap_mib", "rss_mib",
        "min_free_percent", "wall_seconds", "tool_manifest_sha256",
    }
    if bank.seed_mode == "discovery_only":
        config_fields.add("role")
    config = exact_fields(config_value, config_fields, "unseeded configuration")
    for field in (
        "schema_version", "va_start", "va_end", "analysis_timeout_seconds", "max_cpu",
        "heap_mib", "rss_mib", "min_free_percent", "wall_seconds",
    ):
        integer(config[field], f"configuration {field}", 0xFFFF_FFFF)
    expected_config = {
        "schema": "fn64.ghidra-bank-config", "schema_version": 1, "mode": "unseeded",
        "bank": bank.name, "va_start": bank.va_start, "va_end": bank.va_end,
        "base_seed": None if bank.seed_mode == "discovery_only" else bank.base_seed,
        "snapshot_seed": None, "loader": "BinaryLoader", "processor": "MIPS:BE:64:64-32addr",
        "cspec": "o32", "ghidra_version": tool["tool_version"],
        "analysis_timeout_seconds": 120, "max_cpu": 1, "heap_mib": 1024,
        "rss_mib": 2048, "min_free_percent": 40, "wall_seconds": 180,
        "tool_manifest_sha256": measured["unseeded_tool_manifest_sha256"],
    }
    if bank.seed_mode == "discovery_only":
        expected_config["role"] = "candidate_only"
    if config != expected_config:
        fail("unseeded configuration does not match queued bank")

    _, orchestration_value = read_json(
        retained / "tool-artifacts/orchestration.json",
        MIB,
        "orchestration manifest",
        private=True,
    )
    orchestration = exact_fields(
        orchestration_value, {"schema", "schema_version", "artifacts"}, "orchestration manifest"
    )
    if orchestration["schema"] != "fn64.ghidra-orchestration-artifacts" or type(orchestration["schema_version"]) is not int or orchestration["schema_version"] != 1:
        fail("invalid orchestration manifest")
    expected_orchestration_order = [
        "tool-artifacts/ingest_tool_claims",
        "tool-artifacts/manifest-ghidra-distribution.py",
        "tool-artifacts/memory-guard.zsh",
        "tool-artifacts/run-snapshot-bank.sh",
        "tool-artifacts/stage_snapshot_bank",
    ]
    expected_orchestration = set(expected_orchestration_order)
    orchestration_artifacts = orchestration["artifacts"]
    if not isinstance(orchestration_artifacts, list) or len(orchestration_artifacts) != len(expected_orchestration):
        fail("orchestration manifest has wrong artifact count")
    if [artifact.get("path") if isinstance(artifact, dict) else None for artifact in orchestration_artifacts] != expected_orchestration_order:
        fail("orchestration manifest artifact order is not canonical")
    orchestration_hashes = {}
    for index, artifact_value in enumerate(orchestration_artifacts):
        artifact = exact_fields(
            artifact_value, {"path", "byte_length", "sha256"}, f"orchestration artifact {index}"
        )
        relative = artifact["path"]
        if relative not in expected_orchestration or relative in orchestration_hashes:
            fail("orchestration manifest has an unexpected or duplicate path")
        _, length, actual = hash_regular(retained / relative, 64 * MIB, relative, private=True)
        if length == 0:
            fail(f"runner retained empty orchestration artifact {relative}")
        if integer(artifact["byte_length"], f"{relative} length", 64 * MIB) != length or digest(artifact["sha256"], relative) != actual:
            fail(f"orchestration manifest does not bind {relative}")
        orchestration_hashes[relative] = actual
    common_artifacts = {
        path: sha
        for path, sha in artifact_hashes.items()
        if path != "tool-artifacts/Fn64SeedFunctions.java"
    }
    common_cohort = {
        "distribution_sha256": measured["ghidra_distribution_manifest_sha256"],
        "distribution_file_count": len(distribution["files"]),
        "tool_version": tool["tool_version"],
        "tool_artifacts": common_artifacts,
        "orchestration_artifacts": orchestration_hashes,
    }
    common_cohort_sha = hashlib.sha256(
        json.dumps(common_cohort, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()

    return RunnerSuccess(
        retained.name,
        hashlib.sha256(receipt_bytes).hexdigest(),
        measured["tool_claims_sha256"],
        measured["ghidra_distribution_manifest_sha256"],
        measured["unseeded_tool_manifest_sha256"],
        common_cohort_sha,
        len(distribution["files"]),
    )


def capture_runner(
    process: subprocess.Popen,
    stdout_path: Path,
    stderr_path: Path,
    limit: int,
    deadline: float,
    stop,
    termination_grace_seconds: int,
) -> tuple[int, bool, bool]:
    selector = selectors.DefaultSelector()
    selector.register(process.stdout, selectors.EVENT_READ, [stdout_path, 0])
    selector.register(process.stderr, selectors.EVENT_READ, [stderr_path, 0])
    exceeded = False
    timed_out = False
    streams = {}
    for path in (stdout_path, stderr_path):
        descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        streams[path] = os.fdopen(descriptor, "wb")
    termination_started = None
    forced_cleanup_started = None

    def terminate(signum: int) -> None:
        nonlocal termination_started
        if process.poll() is None and termination_started is None:
            process.send_signal(signum)
            termination_started = time.monotonic()

    try:
        while selector.get_map():
            if forced_cleanup_started is not None and time.monotonic() - forced_cleanup_started >= 1:
                for key in list(selector.get_map().values()):
                    selector.unregister(key.fileobj)
                    key.fileobj.close()
                continue
            if process.poll() is not None and termination_started is None:
                # A descendant can inherit the pipes after the runner exits.
                # Bound that drain; the queue cannot otherwise distinguish it
                # from an orphan that will hold the descriptors forever.
                termination_started = time.monotonic()
            if time.monotonic() >= deadline and process.poll() is None:
                timed_out = True
                terminate(signal.SIGTERM)
            for key, _ in selector.select(timeout=0.1):
                block = os.read(key.fileobj.fileno(), 64 * 1024)
                path, measured = key.data
                if not block:
                    selector.unregister(key.fileobj)
                    continue
                keep = max(0, limit - measured)
                streams[path].write(block[:keep])
                measured += len(block)
                key.data[1] = measured
                if measured > limit and not exceeded:
                    exceeded = True
                    terminate(signal.SIGTERM)
            if stop[0] is not None and process.poll() is None and not stop[1]:
                terminate(stop[0])
                stop[1] = True
            if termination_started is not None and time.monotonic() - termination_started >= termination_grace_seconds:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                forced_cleanup_started = time.monotonic()
                termination_started = float("inf")
        return process.wait(), exceeded, timed_out
    finally:
        selector.close()
        if process.stdout is not None and not process.stdout.closed:
            process.stdout.close()
        if process.stderr is not None and not process.stderr.closed:
            process.stderr.close()
        for stream in streams.values():
            stream.flush()
            os.fsync(stream.fileno())
            stream.close()


def tree_bytes(root: Path, limit: int) -> int:
    total = 0
    for current, directories, files in os.walk(root, topdown=True, followlinks=False, onerror=lambda error: fail(f"cannot traverse attempt: {error}")):
        for name in directories:
            mode = (Path(current) / name).lstat().st_mode
            if not stat.S_ISDIR(mode) or stat.S_ISLNK(mode):
                fail("attempt contains a symlink or special directory")
        for name in files:
            metadata = (Path(current) / name).lstat()
            if not stat.S_ISREG(metadata.st_mode) or stat.S_ISLNK(metadata.st_mode):
                fail("attempt contains a symlink or special file")
            total += metadata.st_size
            if total > limit:
                return total
    return total


def run_queue(
    input_path: Path,
    output_path: Path,
    *,
    limits: QueueLimits = QueueLimits(),
    lock_path: Path | None = None,
    runner_path: Path | None = None,
) -> int:
    for field in (
        "max_launches", "max_wall_seconds", "max_attempts_per_bank",
        "max_ordinary_failures", "max_log_bytes", "max_attempt_bytes",
        "min_free_disk_bytes", "termination_grace_seconds",
    ):
        value = getattr(limits, field)
        if type(value) is not int or value <= 0:
            fail(f"queue limit {field} must be a positive integer")
    if limits.max_attempt_bytes <= ATTEMPT_RESULT_RESERVE_BYTES:
        fail("queue attempt cap is too small for its terminal receipt reserve")
    source = canonical_private_directory(input_path, "snapshot workspace")
    output = canonical_private_directory(output_path, "queue workspace")
    if source == output or source in output.parents or output in source.parents:
        fail("snapshot and queue workspaces must be disjoint")
    manifest_bytes, manifest_sha, manifest, banks = validate_manifest(source)
    del manifest_bytes
    repo = Path(__file__).resolve().parents[2]
    queue_script = Path(__file__).resolve(strict=True)
    runner = runner_path if runner_path is not None else repo / "tools/ghidra/run-snapshot-bank.sh"
    stage = Path(os.environ.get("FN64_STAGE_SNAPSHOT_BANK", ""))
    ingest = Path(os.environ.get("FN64_INGEST_TOOL_CLAIMS", ""))
    for path, label in ((queue_script, "queue"), (runner, "runner"), (stage, "stage helper"), (ingest, "ingest helper")):
        if not path.is_absolute() or not path.is_file() or path.is_symlink() or not os.access(path, os.X_OK):
            fail(f"{label} must be an absolute executable regular non-symlink file")
    if lock_path is None:
        lock_path = Path(tempfile.gettempdir()).resolve() / f"fn64-ghidra-snapshot-workspace-{os.getuid()}.lock"
    lock_fd = acquire_lock(lock_path)
    try:
        request, request_sha = ensure_request(output, manifest_sha, manifest["normalized_rom_sha256"], queue_script, runner, stage, ingest, limits)
        allowed_output = {"queue-request.json", "banks", "queue-receipt.json"}
        if any(entry.name not in allowed_output for entry in output.iterdir()):
            fail("queue workspace contains an unknown top-level entry")
        banks_root = output / "banks"
        ensure_private_subdirectory(banks_root, "queue banks directory")
        expected_bank_names = {f"{bank.index:06}" for bank in banks}
        if any(entry.name not in expected_bank_names for entry in banks_root.iterdir()):
            fail("queue banks directory contains an unknown entry")
        stop = [None, False]
        previous_handlers = {}
        def request_stop(signum, _frame):
            stop[0] = signum
        for signum in (signal.SIGHUP, signal.SIGINT, signal.SIGTERM):
            previous_handlers[signum] = signal.signal(signum, request_stop)
        started = time.monotonic()
        launches = 0
        ordinary_failures = 0
        completed = []
        common_cohort_sha = None
        common_distribution_sha = None
        common_distribution_file_count = None
        mode_tool_sha = {"discovery_only": None, "base_only": None}

        def admit_cohort(bank: Bank, success: RunnerSuccess) -> None:
            nonlocal common_cohort_sha, common_distribution_sha, common_distribution_file_count
            if common_cohort_sha is None:
                common_cohort_sha = success.common_cohort_sha256
                common_distribution_sha = success.distribution_sha256
                common_distribution_file_count = success.distribution_file_count
            elif common_cohort_sha != success.common_cohort_sha256:
                fail("successful banks do not share one Ghidra/orchestration cohort")
            mode = "discovery_only" if bank.seed_mode == "discovery_only" else "base_only"
            if mode_tool_sha[mode] is None:
                mode_tool_sha[mode] = success.unseeded_tool_sha256
            elif mode_tool_sha[mode] != success.unseeded_tool_sha256:
                fail(f"successful {mode} banks do not share one tool manifest")

        try:
            for bank in banks:
                bank_root = banks_root / f"{bank.index:06}"
                attempts_root = bank_root / "attempts"
                ensure_private_subdirectory(bank_root, f"bank {bank.index} queue directory")
                ensure_private_subdirectory(attempts_root, f"bank {bank.index} attempts directory")
                allowed_bank_entries = {"attempts", "skip.json"} if bank.seed_mode == "ineligible" else {"attempts"}
                if any(entry.name not in allowed_bank_entries for entry in bank_root.iterdir()):
                    fail(f"bank {bank.index} queue directory contains an unknown entry")
                if bank.seed_mode == "ineligible":
                    skip = {"schema": "fn64.ghidra-snapshot-workspace-skip", "schema_version": 1, "reason": "no_proven_owner", **stable_bank_value(bank, request_sha, manifest_sha, 0)}
                    path = bank_root / "skip.json"
                    encoded = (json.dumps(skip, sort_keys=True, separators=(",", ":")) + "\n").encode()
                    if path.exists():
                        existing, _, _ = hash_regular(path, MIB, "skip receipt", retain=True, private=True)
                        if existing != encoded:
                            fail(f"bank {bank.index} skip receipt mismatch")
                    else:
                        publish_new(path, skip)
                    completed.append({"index": bank.index, "state": "skipped", "receipt_sha256": hashlib.sha256(encoded).hexdigest()})
                    continue
                existing_attempts = sorted(attempts_root.iterdir())
                expected_names = [f"{number:06}" for number in range(1, len(existing_attempts) + 1)]
                if [path.name for path in existing_attempts] != expected_names:
                    fail(f"bank {bank.index} attempt numbers are not contiguous")
                success_result = None
                success_digest = None
                for attempt_path in existing_attempts:
                    metadata = attempt_path.lstat()
                    if not stat.S_ISDIR(metadata.st_mode) or stat.S_ISLNK(metadata.st_mode) or metadata.st_uid != os.getuid() or stat.S_IMODE(metadata.st_mode) != 0o700:
                        fail(f"bank {bank.index} has invalid attempt namespace")
                    if tree_bytes(attempt_path, limits.max_attempt_bytes) > limits.max_attempt_bytes:
                        fail(f"bank {bank.index} retained attempt exceeds the output cap")
                    allowed_attempt_entries = {"runner-workspace", "stdout.log", "stderr.log", "result.json"}
                    if any(entry.name not in allowed_attempt_entries for entry in attempt_path.iterdir()):
                        fail(f"bank {bank.index} attempt contains an unknown entry")
                    result_path = attempt_path / "result.json"
                    if not result_path.exists():
                        stdout = attempt_path / "stdout.log"
                        stderr = attempt_path / "stderr.log"
                        abandoned = {"schema": "fn64.ghidra-snapshot-workspace-attempt", "schema_version": 1, "state": "failure", "failure_class": "abandoned_before_terminal_receipt", "runner_exit_status": None, "runner_attempt": None, "runner_receipt_sha256": None, "tool_claims_sha256": None, "ghidra_distribution_manifest_sha256": None, "unseeded_tool_manifest_sha256": None, "common_cohort_sha256": None, "stop_scheduling": False, "stdout": file_identity(stdout, "abandoned stdout") if stdout.exists() else None, "stderr": file_identity(stderr, "abandoned stderr") if stderr.exists() else None, **stable_bank_value(bank, request_sha, manifest_sha, int(attempt_path.name))}
                        publish_new(result_path, abandoned)
                    result, runner_success, result_sha = validate_attempt_result(
                        attempt_path, bank, request_sha, manifest_sha
                    )
                    if result["state"] == "failure" and not result["stop_scheduling"]:
                        ordinary_failures += 1
                    if runner_success is not None:
                        if success_result is not None:
                            fail(f"bank {bank.index} has multiple successful attempts")
                        success_result = result
                        success_digest = result_sha
                        admit_cohort(bank, runner_success)
                if success_result is not None:
                    completed.append({"index": bank.index, "state": "success", "receipt_sha256": success_digest})
                    continue
                if len(existing_attempts) >= limits.max_attempts_per_bank:
                    fail(f"bank {bank.index} exhausted its attempt cap")
                if ordinary_failures >= limits.max_ordinary_failures:
                    return 1
                if stop[0] is not None or launches >= limits.max_launches or time.monotonic() - started >= limits.max_wall_seconds:
                    return 75 if stop[0] is None else 128 + stop[0]
                if os.statvfs(output).f_bavail * os.statvfs(output).f_frsize < limits.min_free_disk_bytes:
                    fail("queue workspace has insufficient free disk")
                # Revalidate immutable producer inputs immediately before launch.
                _, _, current_manifest_sha = hash_regular(source / "snapshot-workspace.json", MAX_MANIFEST_BYTES, "snapshot manifest", private=True)
                if current_manifest_sha != manifest_sha:
                    fail("snapshot manifest changed during queue run")
                _, length, sha = hash_regular(source / bank.bank_artifact, MAX_BANK_BYTES, "bank artifact", private=True)
                _, snapshot_length, snapshot_sha, program_sha = hash_regular(source / bank.snapshot_artifact, MAX_SNAPSHOT_BYTES, "snapshot artifact", program_wire=True, private=True)
                if (length, sha, snapshot_length, snapshot_sha, program_sha) != (bank.bank_length, bank.bank_sha256, bank.snapshot_length, bank.snapshot_sha256, bank.program_snapshot_sha256):
                    fail(f"bank {bank.index} input changed during queue run")
                attempt_number = len(existing_attempts) + 1
                attempt_path = attempts_root / f"{attempt_number:06}"
                attempt_path.mkdir(mode=0o700)
                runner_workspace = attempt_path / "runner-workspace"
                ensure_private_subdirectory(runner_workspace, "runner workspace")
                environment = os.environ.copy()
                runner_args = [
                    runner,
                    "--discovery-only" if bank.seed_mode == "discovery_only" else "--unseeded-only",
                    source / bank.snapshot_artifact,
                    bank.name,
                    source / bank.bank_artifact,
                    runner_workspace,
                ]
                if bank.seed_mode != "discovery_only":
                    runner_args.append(f"0x{bank.base_seed:08x}")
                try:
                    process = subprocess.Popen(
                        runner_args,
                        stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=environment,
                        pass_fds=(lock_fd,), start_new_session=True,
                    )
                except OSError as error:
                    fail(f"cannot launch snapshot-bank runner: {error}")
                launches += 1
                status, log_exceeded, wall_exceeded = capture_runner(
                    process,
                    attempt_path / "stdout.log",
                    attempt_path / "stderr.log",
                    limits.max_log_bytes,
                    started + limits.max_wall_seconds,
                    stop,
                    limits.termination_grace_seconds,
                )
                stdout_identity = file_identity(attempt_path / "stdout.log", "runner stdout")
                stderr_identity = file_identity(attempt_path / "stderr.log", "runner stderr")
                failure_class = None
                runner_success = None
                if status == 0 and not log_exceeded and not wall_exceeded and stop[0] is None:
                    try:
                        runner_success = validate_runner_success(runner_workspace, bank)
                    except QueueError:
                        failure_class = "invalid_runner_completion"
                elif stop[0] is not None:
                    failure_class = "queue_interrupted"
                elif wall_exceeded:
                    failure_class = "queue_wall_cap"
                elif log_exceeded:
                    failure_class = "log_cap"
                else:
                    failure_class = "runner_exit"
                payload_limit = limits.max_attempt_bytes - ATTEMPT_RESULT_RESERVE_BYTES
                if tree_bytes(attempt_path, payload_limit) > payload_limit:
                    failure_class = "attempt_output_cap"
                    runner_success = None
                # A retained success is terminal only if its immutable producer inputs still match.
                _, _, current_manifest_sha = hash_regular(source / "snapshot-workspace.json", MAX_MANIFEST_BYTES, "snapshot manifest", private=True)
                _, length, sha = hash_regular(source / bank.bank_artifact, MAX_BANK_BYTES, "bank artifact", private=True)
                _, snapshot_length, snapshot_sha, program_sha = hash_regular(source / bank.snapshot_artifact, MAX_SNAPSHOT_BYTES, "snapshot artifact", program_wire=True, private=True)
                if current_manifest_sha != manifest_sha or (length, sha, snapshot_length, snapshot_sha, program_sha) != (bank.bank_length, bank.bank_sha256, bank.snapshot_length, bank.snapshot_sha256, bank.program_snapshot_sha256):
                    fail(f"bank {bank.index} input changed while its runner was active")
                for path, label in ((queue_script, "queue"), (runner, "runner"), (stage, "stage"), (ingest, "ingest")):
                    if not path.is_file() or path.is_symlink() or not os.access(path, os.X_OK):
                        fail(f"{label} helper changed type or executability while runner was active")
                    if file_identity(path, label) != request["tools"][label]:
                        fail(f"{label} helper changed while runner was active")
                state = "success" if failure_class is None else "failure"
                if state == "success" and runner_success is None:
                    fail("internal queue error: success lacks validated runner receipt")
                result = {"schema": "fn64.ghidra-snapshot-workspace-attempt", "schema_version": 1, "state": state, "failure_class": failure_class, "runner_exit_status": status, "runner_attempt": runner_success.attempt_name if runner_success else None, "runner_receipt_sha256": runner_success.receipt_sha256 if runner_success else None, "tool_claims_sha256": runner_success.claims_sha256 if runner_success else None, "ghidra_distribution_manifest_sha256": runner_success.distribution_sha256 if runner_success else None, "unseeded_tool_manifest_sha256": runner_success.unseeded_tool_sha256 if runner_success else None, "common_cohort_sha256": runner_success.common_cohort_sha256 if runner_success else None, "stop_scheduling": stop[0] is not None or failure_class in {"queue_interrupted", "queue_wall_cap", "log_cap", "attempt_output_cap"}, "stdout": stdout_identity, "stderr": stderr_identity, **stable_bank_value(bank, request_sha, manifest_sha, attempt_number)}
                result_bytes = publish_new(attempt_path / "result.json", result)
                if state == "success":
                    admit_cohort(bank, runner_success)
                    completed.append({"index": bank.index, "state": "success", "receipt_sha256": hashlib.sha256(result_bytes).hexdigest()})
                else:
                    ordinary_failures += 1
                    if result["stop_scheduling"] or ordinary_failures >= limits.max_ordinary_failures:
                        return 128 + stop[0] if stop[0] is not None else 1
            if len(completed) == len(banks):
                _, final_manifest_sha, final_manifest, final_banks = validate_manifest(source)
                if final_manifest_sha != manifest_sha or final_manifest != manifest or final_banks != banks:
                    fail("snapshot workspace changed before terminal queue receipt")
                for path, label in ((queue_script, "queue"), (runner, "runner"), (stage, "stage"), (ingest, "ingest")):
                    if not path.is_file() or path.is_symlink() or not os.access(path, os.X_OK) or file_identity(path, label) != request["tools"][label]:
                        fail(f"{label} helper changed before terminal queue receipt")
                receipt = {"schema": "fn64.ghidra-snapshot-workspace-receipt", "schema_version": 1, "state": "candidate_queue_complete", "execution_mode": "candidate-only-sequential", "queue_request_sha256": request_sha, "source_manifest_sha256": manifest_sha, "normalized_rom_sha256": manifest["normalized_rom_sha256"], "cohort": {"common_sha256": common_cohort_sha, "ghidra_distribution_manifest_sha256": common_distribution_sha, "ghidra_distribution_file_count": common_distribution_file_count, "tool_artifact_scope": "all-ghidra-install-regular-files,jdk-java,fn64-analysis-scripts,and-bound-orchestration-helpers" if common_cohort_sha is not None else None, "mode_tool_manifest_sha256": mode_tool_sha}, "banks": completed}
                encoded = (json.dumps(receipt, sort_keys=True, separators=(",", ":")) + "\n").encode()
                receipt_path = output / "queue-receipt.json"
                if receipt_path.exists():
                    existing, _ = read_json(receipt_path, MIB, "queue receipt", private=True)
                    if existing != encoded:
                        fail("queue receipt does not match validated retained attempts")
                else:
                    publish_new(receipt_path, receipt)
                return 0
            return 1
        finally:
            for signum, handler in previous_handlers.items():
                signal.signal(signum, handler)
    finally:
        os.close(lock_fd)


def usage() -> str:
    return "usage: tools/ghidra/run-snapshot-workspace.py SNAPSHOT_WORKSPACE QUEUE_WORKSPACE"


def main(arguments: list[str]) -> int:
    if len(arguments) != 2:
        print(usage(), file=sys.stderr)
        return 2
    try:
        os.umask(0o077)
        return run_queue(Path(arguments[0]), Path(arguments[1]))
    except QueueError as error:
        print(f"ghidra snapshot-workspace: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
