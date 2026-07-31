#!/usr/bin/env python3
"""Orchestrator-scoped cold-training fold with delayed held-out admission."""

from __future__ import annotations

import argparse
import ctypes
import errno
import hashlib
import json
import os
import re
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

SCHEMA_PREPARE = "fn64.loo-prepare-input.v1"
SCHEMA_TRAINING = "fn64.loo-training-receipt.v1"
SCHEMA_MECHANISM = "fn64.discovery-mechanism.v1"
SCHEMA_FREEZE = "fn64.loo-freeze.v1"
SCHEMA_ADMISSION = "fn64.loo-heldout-key-admission.v1"
SCHEMA_HELDOUT = "fn64.loo-heldout-receipt.v1"
VALIDATION_SUMMARY_SCHEMA = "fn64.known-function-attribution-validation.v1"
HEX256 = re.compile(r"[0-9a-f]{64}\Z")
ID = re.compile(r"[a-z0-9][a-z0-9_-]{0,63}\Z")
MAX_JSON_BYTES = 16 * 1024 * 1024
MAX_ATTRIBUTION_BYTES = 128 * 1024 * 1024
MAX_SUBPROCESS_OUTPUT_BYTES = 1024 * 1024
MAX_ROM_BYTES = 64 * 1024 * 1024
MAX_DUMP_BYTES = 128 * 1024 * 1024
DEFAULT_MAX_RSS_MIB = 2048
DEFAULT_MIN_FREE_PERCENT = 40
RESOURCE_SAMPLER = "macos-exact-pgid-ps-memory-pressure.v1"
RESOURCE_POLL_MILLISECONDS = 1000
BINARIES = ("rom_identity", "produce_snapshot_workspace", "validate_training_workspace", "attribute_known_functions")


class FoldError(RuntimeError):
    pass


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def canonical_bytes(value: Any) -> bytes:
    return (json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False) + "\n").encode()


def receipt_bytes(value: dict[str, Any]) -> bytes:
    body = dict(value)
    body["canonical_sha256"] = sha256_bytes(canonical_bytes(value))
    return canonical_bytes(body)


def exact_keys(value: dict[str, Any], required: set[str], optional: set[str], label: str) -> None:
    missing = required - set(value)
    unknown = set(value) - required - optional
    if missing or unknown:
        raise FoldError(f"{label} fields differ: missing={sorted(missing)} unknown={sorted(unknown)}")


def require_object(value: Any, label: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise FoldError(f"{label} must be a JSON object")
    return value


def require_string(value: Any, label: str) -> str:
    if not isinstance(value, str) or not value:
        raise FoldError(f"{label} must be a nonempty string")
    return value


def require_id(value: Any, label: str) -> str:
    value = require_string(value, label)
    if not ID.fullmatch(value):
        raise FoldError(f"{label} is not a path-free lowercase identifier")
    return value


def require_digest(value: Any, label: str) -> str:
    value = require_string(value, label)
    if not HEX256.fullmatch(value):
        raise FoldError(f"{label} must be a lowercase SHA-256")
    return value


def canonical_regular(path_text: Any, label: str, *, executable: bool = False, max_bytes: int | None = None) -> Path:
    path = Path(require_string(path_text, label))
    if not path.is_absolute():
        raise FoldError(f"{label} must be absolute")
    try:
        info = path.lstat()
    except OSError as error:
        raise FoldError(f"inspecting {label}: {error}") from error
    if path.is_symlink() or not stat.S_ISREG(info.st_mode) or path.resolve() != path:
        raise FoldError(f"{label} must be a canonical no-symlink regular file")
    if executable and not os.access(path, os.X_OK):
        raise FoldError(f"{label} must be executable")
    if max_bytes is not None and info.st_size > max_bytes:
        raise FoldError(f"{label} exceeds its byte bound")
    return path


def canonical_directory(path_text: Any, label: str, *, private: bool = False) -> Path:
    path = Path(require_string(path_text, label))
    if not path.is_absolute() or path.is_symlink() or not path.is_dir() or path.resolve() != path:
        raise FoldError(f"{label} must be a canonical no-symlink absolute directory")
    if private and stat.S_IMODE(path.stat().st_mode) != 0o700:
        raise FoldError(f"{label} must have mode 0700")
    return path


def outside_git(path: Path, label: str) -> None:
    for ancestor in (path, *path.parents):
        if (ancestor / ".git").exists():
            raise FoldError(f"{label} must be outside a Git worktree")


def private_run(path_text: str) -> Path:
    run = canonical_directory(path_text, "run", private=True)
    outside_git(run, "run")
    return run


def make_private(path: Path) -> None:
    try:
        path.mkdir(mode=0o700)
    except FileExistsError as error:
        raise FoldError(f"refusing existing directory {path.name!r}") from error
    if stat.S_IMODE(path.stat().st_mode) != 0o700:
        raise FoldError(f"created directory {path.name!r} is not mode 0700")


def staged_directory(final: Path, label: str) -> Path:
    if not final.is_absolute() or final.name in ("", ".", ".."):
        raise FoldError(f"{label} must be an absent canonical absolute child")
    parent = canonical_directory(str(final.parent), f"{label} parent")
    outside_git(parent, label)
    final = parent / final.name
    if final.exists() or final.is_symlink():
        raise FoldError(f"{label} must be absent")
    stage = Path(tempfile.mkdtemp(prefix=f".{final.name}.stage-", dir=parent))
    stage.chmod(0o700)
    return stage


def publish_directory(stage: Path, final: Path) -> None:
    rename_directory_exclusive(stage, final)
    descriptor = os.open(final.parent, os.O_RDONLY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def rename_directory_exclusive(stage: Path, final: Path) -> None:
    try:
        renameatx_np = ctypes.CDLL(None, use_errno=True).renameatx_np
    except AttributeError as error:
        raise FoldError("exclusive directory publication is unavailable") from error
    renameatx_np.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint]
    renameatx_np.restype = ctypes.c_int
    at_fdcwd = -2
    rename_excl = 0x00000004
    if renameatx_np(at_fdcwd, os.fsencode(stage), at_fdcwd, os.fsencode(final), rename_excl) != 0:
        error_number = ctypes.get_errno()
        if error_number in (errno.EEXIST, errno.ENOTEMPTY):
            raise FoldError(f"refusing to overwrite directory {final.name!r}")
        raise FoldError(f"exclusive directory publication failed ({error_number})")


def publish_new(path: Path, data: bytes) -> str:
    if len(data) > MAX_JSON_BYTES or path.exists() or path.is_symlink():
        raise FoldError(f"refusing invalid or existing receipt {path.name!r}")
    temporary = path.with_name(f".{path.name}.fn64-tmp-{os.getpid()}")
    descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o600)
    try:
        view = memoryview(data)
        while view:
            written = os.write(descriptor, view)
            if written <= 0:
                raise FoldError("receipt publication made no write progress")
            view = view[written:]
        os.fsync(descriptor)
        os.close(descriptor)
        descriptor = -1
        os.link(temporary, path)
    except FileExistsError as error:
        raise FoldError(f"refusing to overwrite receipt {path.name!r}") from error
    finally:
        if descriptor >= 0:
            os.close(descriptor)
        temporary.unlink(missing_ok=True)
    if stat.S_IMODE(path.stat().st_mode) != 0o600:
        raise FoldError(f"published receipt {path.name!r} is not mode 0600")
    directory = os.open(path.parent, os.O_RDONLY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)
    return sha256_bytes(data)


def read_json(path: Path, label: str, *, canonical: bool = False, max_bytes: int = MAX_JSON_BYTES) -> tuple[dict[str, Any], bytes]:
    path = canonical_regular(str(path), label, max_bytes=max_bytes)
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(path, flags)
    try:
        chunks = bytearray()
        while chunk := os.read(descriptor, 1024 * 1024):
            chunks.extend(chunk)
            if len(chunks) > max_bytes:
                raise FoldError(f"{label} exceeds its byte bound")
    finally:
        os.close(descriptor)
    data = bytes(chunks)
    try:
        value = require_object(json.loads(data), label)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise FoldError(f"parsing {label}: invalid JSON") from error
    if canonical and canonical_bytes(value) != data:
        raise FoldError(f"{label} is not canonical JSON")
    return value, data


def copy_stable_input(source: Path, destination: Path, *, max_bytes: int, executable: bool = False) -> str:
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    source_fd = os.open(source, flags)
    destination_fd = -1
    digest = hashlib.sha256()
    try:
        before = os.fstat(source_fd)
        if not stat.S_ISREG(before.st_mode) or before.st_size > max_bytes:
            raise FoldError("input changed type or exceeds its byte bound")
        destination_fd = os.open(destination, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o700 if executable else 0o600)
        total = 0
        while chunk := os.read(source_fd, 1024 * 1024):
            total += len(chunk)
            if total > max_bytes:
                raise FoldError("input exceeds its byte bound while staging")
            digest.update(chunk)
            view = memoryview(chunk)
            while view:
                written = os.write(destination_fd, view)
                if written <= 0:
                    raise FoldError("staged input copy made no write progress")
                view = view[written:]
        os.fsync(destination_fd)
        after = os.fstat(source_fd)
        current = source.lstat()
        identity_before = (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns)
        identity_after = (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns)
        if identity_after != identity_before or (current.st_dev, current.st_ino) != identity_before[:2]:
            raise FoldError("input changed while it was staged")
    finally:
        os.close(source_fd)
        if destination_fd >= 0:
            os.close(destination_fd)
    return digest.hexdigest()


def sha256_stable_file(path: Path, label: str, *, max_bytes: int) -> str:
    path = canonical_regular(str(path), label, max_bytes=max_bytes)
    descriptor = os.open(path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0))
    digest = hashlib.sha256()
    try:
        before = os.fstat(descriptor)
        total = 0
        while chunk := os.read(descriptor, 1024 * 1024):
            total += len(chunk)
            if total > max_bytes:
                raise FoldError(f"{label} exceeds its byte bound while hashing")
            digest.update(chunk)
        after = os.fstat(descriptor)
        current = path.lstat()
        identity_before = (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns, before.st_ctime_ns)
        identity_after = (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns, after.st_ctime_ns)
        if identity_after != identity_before or (current.st_dev, current.st_ino) != identity_before[:2]:
            raise FoldError(f"{label} changed while it was hashed")
    finally:
        os.close(descriptor)
    return digest.hexdigest()


def source_binaries(bin_dir_text: str) -> dict[str, Path]:
    root = canonical_directory(bin_dir_text, "binary directory")
    return {name: canonical_regular(str(root / name), f"binary {name}", executable=True) for name in BINARIES}


def stage_binaries(bin_dir_text: str, destination: Path) -> tuple[dict[str, Path], dict[str, str]]:
    make_private(destination)
    paths: dict[str, Path] = {}
    identities: dict[str, str] = {}
    for name, source in source_binaries(bin_dir_text).items():
        paths[name] = destination / name
        identities[name] = copy_stable_input(source, paths[name], max_bytes=MAX_DUMP_BYTES, executable=True)
    return paths, identities


def kill_group(process: subprocess.Popen[bytes]) -> None:
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    process.wait()


def subprocess_limits(max_rss_mib: int, min_free_percent: int) -> dict[str, Any]:
    if not isinstance(max_rss_mib, int) or isinstance(max_rss_mib, bool):
        raise FoldError("maximum process-group RSS must be an integer")
    if not isinstance(min_free_percent, int) or isinstance(min_free_percent, bool):
        raise FoldError("minimum system-free percentage must be an integer")
    if max_rss_mib < 1 or max_rss_mib > 1024 * 1024:
        raise FoldError("maximum process-group RSS must be between 1 and 1048576 MiB")
    if min_free_percent < 0 or min_free_percent > 100:
        raise FoldError("minimum system-free percentage must be between 0 and 100")
    return {"max_process_group_rss_mib": max_rss_mib, "min_system_free_percent": min_free_percent,
            "poll_milliseconds": RESOURCE_POLL_MILLISECONDS, "sampler": RESOURCE_SAMPLER}


def validate_subprocess_limits(value: Any, label: str) -> dict[str, Any]:
    value = require_object(value, label)
    exact_keys(value, {"max_process_group_rss_mib", "min_system_free_percent", "poll_milliseconds", "sampler"}, set(), label)
    if value["sampler"] != RESOURCE_SAMPLER:
        raise FoldError(f"{label} sampler is unsupported")
    if value["poll_milliseconds"] != RESOURCE_POLL_MILLISECONDS:
        raise FoldError(f"{label} polling cadence is unsupported")
    return subprocess_limits(value["max_process_group_rss_mib"], value["min_system_free_percent"])


def command_output(argv: list[str], label: str) -> str:
    try:
        result = subprocess.run(argv, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                env={"PATH": "/usr/bin:/bin"}, timeout=2, check=False)
    except (OSError, subprocess.TimeoutExpired) as error:
        raise FoldError(f"resource sampling failed: {label}") from error
    if result.returncode != 0 or len(result.stdout) > MAX_SUBPROCESS_OUTPUT_BYTES:
        raise FoldError(f"resource sampling failed: {label}")
    try:
        return result.stdout.decode("ascii")
    except UnicodeDecodeError as error:
        raise FoldError(f"resource sampling failed: {label}") from error


def sample_resources(pgid: int | None) -> tuple[int, int, int]:
    snapshot = command_output(["/bin/ps", "-axo", "pid=,pgid=,rss="], "process_table")
    rss_kib = 0
    members = 0
    for line in snapshot.splitlines():
        fields = line.split()
        if len(fields) != 3 or any(not field.isascii() or not field.isdecimal() for field in fields):
            raise FoldError("resource sampling failed: process_table")
        _pid, process_pgid, process_rss = (int(field) for field in fields)
        if pgid is not None and process_pgid == pgid:
            members += 1
            rss_kib += process_rss
    pressure = command_output(["/usr/bin/memory_pressure", "-Q"], "free_memory")
    prefix = "System-wide memory free percentage: "
    candidates = [line[len(prefix):-1] for line in pressure.splitlines() if line.startswith(prefix) and line.endswith("%")]
    if len(candidates) != 1 or not candidates[0].isascii() or not candidates[0].isdecimal():
        raise FoldError("resource sampling failed: free_memory")
    free_percent = int(candidates[0])
    if free_percent < 0 or free_percent > 100:
        raise FoldError("resource sampling failed: free_memory")
    return rss_kib, free_percent, members


def enforce_resources(sample: tuple[int, int, int], limits: dict[str, Any], process: subprocess.Popen[bytes], label: str) -> None:
    rss_kib, free_percent, _members = sample
    if rss_kib > limits["max_process_group_rss_mib"] * 1024:
        kill_group(process)
        raise FoldError(f"{label} failed: memory_rss_limit")
    if free_percent < limits["min_system_free_percent"]:
        kill_group(process)
        raise FoldError(f"{label} failed: memory_free_floor")


def run_bounded(argv: list[str], timeout: int, label: str, scratch: Path, limits: dict[str, Any]) -> str:
    if timeout < 1 or timeout > 7200:
        raise FoldError("timeout must be between 1 and 7200 seconds")
    stdout_path = scratch / f".child-{os.getpid()}-{time.monotonic_ns()}-stdout"
    stderr_path = scratch / f".child-{os.getpid()}-{time.monotonic_ns()}-stderr"
    stdout_fd = os.open(stdout_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    stderr_fd = os.open(stderr_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    process: subprocess.Popen[bytes] | None = None
    try:
        try:
            preflight = sample_resources(None)
        except FoldError:
            raise FoldError(f"{label} failed: resource_sampling") from None
        if preflight[1] < limits["min_system_free_percent"]:
            raise FoldError(f"{label} failed: memory_free_floor")
        process = subprocess.Popen(argv, stdin=subprocess.DEVNULL, stdout=stdout_fd, stderr=stderr_fd,
                                   env={"PATH": os.environ.get("PATH", "/usr/bin:/bin")}, start_new_session=True)
        started = time.monotonic()
        next_resource_sample = started
        while True:
            now = time.monotonic()
            if now - started > timeout:
                kill_group(process)
                raise FoldError(f"{label} failed: timeout")
            if os.fstat(stdout_fd).st_size > MAX_SUBPROCESS_OUTPUT_BYTES or os.fstat(stderr_fd).st_size > MAX_SUBPROCESS_OUTPUT_BYTES:
                kill_group(process)
                raise FoldError(f"{label} failed: output_limit")
            sample = None
            if now >= next_resource_sample:
                try:
                    sample = sample_resources(process.pid)
                except FoldError:
                    kill_group(process)
                    raise FoldError(f"{label} failed: resource_sampling") from None
                enforce_resources(sample, limits, process, label)
                next_resource_sample = now + limits["poll_milliseconds"] / 1000
            returncode = process.poll()
            if returncode is not None:
                try:
                    final_sample = sample_resources(process.pid)
                except FoldError:
                    kill_group(process)
                    raise FoldError(f"{label} failed: resource_sampling") from None
                enforce_resources(final_sample, limits, process, label)
                if final_sample[2] != 0:
                    kill_group(process)
                    raise FoldError(f"{label} failed: child_survivors")
                break
            if sample is not None and sample[2] == 0:
                kill_group(process)
                raise FoldError(f"{label} failed: resource_sampling")
            time.sleep(min(0.05, max(0.0, next_resource_sample - time.monotonic())))
        if os.fstat(stdout_fd).st_size > MAX_SUBPROCESS_OUTPUT_BYTES or os.fstat(stderr_fd).st_size > MAX_SUBPROCESS_OUTPUT_BYTES:
            raise FoldError(f"{label} failed: output_limit")
        if process.returncode != 0:
            raise FoldError(f"{label} failed: child_exit_{process.returncode}")
        os.close(stdout_fd)
        stdout_fd = -1
        return stdout_path.read_text(errors="replace")
    except BaseException:
        if process is not None and process.poll() is None:
            kill_group(process)
        raise
    finally:
        if stdout_fd >= 0:
            os.close(stdout_fd)
        os.close(stderr_fd)
        stdout_path.unlink(missing_ok=True)
        stderr_path.unlink(missing_ok=True)


def parse_prepare(path: Path) -> dict[str, Any]:
    manifest, _ = read_json(path, "prepare manifest")
    exact_keys(manifest, {"schema", "schema_version", "fold_id", "training_ids", "held_out_id", "entries"}, set(), "prepare manifest")
    if manifest["schema"] != SCHEMA_PREPARE or manifest["schema_version"] != 1:
        raise FoldError("unsupported prepare manifest schema")
    fold_id = require_id(manifest["fold_id"], "fold_id")
    held_out = require_id(manifest["held_out_id"], "held_out_id")
    if not isinstance(manifest["training_ids"], list) or not manifest["training_ids"]:
        raise FoldError("training_ids must be a nonempty array")
    training = [require_id(value, "training id") for value in manifest["training_ids"]]
    if training != sorted(set(training)) or held_out in training:
        raise FoldError("training_ids must be unique sorted and exclude held_out_id")
    if not isinstance(manifest["entries"], list):
        raise FoldError("entries must be an array")
    entries = []
    for raw in manifest["entries"]:
        entry = require_object(raw, "prepare entry")
        exact_keys(entry, {"id", "family", "rom_path", "expected_normalized_rom_sha256"}, {"answer_key"}, "prepare entry")
        game_id = require_id(entry["id"], "entry id")
        answer = entry.get("answer_key")
        if game_id == held_out and answer is not None:
            raise FoldError("held-out prepare entry must not contain answer_key")
        if game_id in training and answer is None:
            raise FoldError(f"training entry {game_id!r} requires answer_key")
        parsed_answer = None
        if answer is not None:
            answer = require_object(answer, f"{game_id} answer_key")
            exact_keys(answer, {"format", "path", "expected_sha256", "license_disposition"}, set(), f"{game_id} answer_key")
            if answer["format"] != "fn64.dump-toml.v1":
                raise FoldError("only fn64.dump-toml.v1 answer keys are accepted")
            parsed_answer = {"path": canonical_regular(answer["path"], f"{game_id} answer key", max_bytes=MAX_DUMP_BYTES),
                             "expected_sha256": require_digest(answer["expected_sha256"], f"{game_id} expected answer digest"),
                             "license_disposition": require_id(answer["license_disposition"], f"{game_id} license disposition")}
        entries.append({"id": game_id, "family": require_id(entry["family"], "entry family"),
                        "rom": canonical_regular(entry["rom_path"], f"{game_id} ROM", max_bytes=MAX_ROM_BYTES),
                        "rom_digest": require_digest(entry["expected_normalized_rom_sha256"], f"{game_id} expected ROM digest"), "answer": parsed_answer})
    if [entry["id"] for entry in entries] != sorted([*training, held_out]):
        raise FoldError("entries must be sorted and exactly equal training_ids plus held_out_id")
    return {"fold_id": fold_id, "training_ids": training, "held_out_id": held_out, "entries": entries}


def rom_identity(binary: Path, rom: Path, timeout: int, scratch: Path, limits: dict[str, Any]) -> str:
    out = run_bounded([str(binary), str(rom)], timeout, "ROM identity", scratch, limits)
    try:
        value = require_object(json.loads(out), "ROM identity receipt")
    except json.JSONDecodeError as error:
        raise FoldError("parsing ROM identity receipt: invalid JSON") from error
    exact_keys(value, {"schema", "schema_version", "normalized_rom_sha256", "source_byte_order", "byte_length", "entry_point"}, set(), "ROM identity receipt")
    if value["schema"] != "fn64.rom-identity" or value["schema_version"] != 1:
        raise FoldError("unexpected ROM identity receipt schema")
    return require_digest(value["normalized_rom_sha256"], "normalized ROM digest")


def cold_identity(cold: Path, expected_rom: str) -> dict[str, str]:
    manifest, data = read_json(cold / "snapshot-workspace.json", "cold workspace manifest")
    exact_keys(manifest, {"schema", "schema_version", "state", "open_reason", "normalized_rom_sha256", "discovery", "limits", "snapshot_wire", "aggregate_snapshot_artifact_bytes", "rom_recompilation_complete", "remaining_recompilation_frontier", "intended_use", "cold_training", "banks"}, {"selection"}, "cold workspace manifest")
    if manifest["schema"] != "fn64.snapshot-workspace" or manifest["schema_version"] != 3 or manifest["intended_use"] != "sealed_cold_function_training_input":
        raise FoldError("producer did not emit schema3 cold-training workspace")
    if manifest["normalized_rom_sha256"] != expected_rom:
        raise FoldError("cold workspace ROM digest mismatch")
    training = require_object(manifest["cold_training"], "cold_training receipt")
    exact_keys(training, {"schema_version", "algorithm", "answer_key_present", "candidate_artifact", "candidate_artifact_byte_length", "candidate_artifact_sha256", "scoped_candidate_identities_v3_sha256"}, set(), "cold_training receipt")
    artifact_length = training["candidate_artifact_byte_length"]
    if (training["schema_version"] != 3
            or training["algorithm"] != "fn64.cold-function-training.v3"
            or training["answer_key_present"] is not False
            or training["candidate_artifact"] != "cold-candidates.json"
            or not isinstance(artifact_length, int) or isinstance(artifact_length, bool)
            or artifact_length < 0):
        raise FoldError("cold workspace has a stale or malformed cold-training receipt")
    require_digest(training["candidate_artifact_sha256"], "candidate artifact digest")
    snapshot_wire = require_object(manifest["snapshot_wire"], "snapshot wire receipt")
    exact_keys(snapshot_wire, {"schema_version", "authority", "duplicates_fact_db_per_bank", "remaining_large_rom_frontier"}, set(), "snapshot wire receipt")
    if (snapshot_wire["schema_version"] != 5
            or snapshot_wire["authority"] != "diagnostic_only"
            or snapshot_wire["duplicates_fact_db_per_bank"] is not False
            or snapshot_wire["remaining_large_rom_frontier"] != "streaming_v5"):
        raise FoldError("cold workspace does not bind the current snapshot wire")
    return {"normalized_rom_sha256": expected_rom, "cold_manifest_sha256": sha256_bytes(data),
            "candidate_identity_v3_sha256": require_digest(training.get("scoped_candidate_identities_v3_sha256"), "candidate identity v3 digest")}


def validate_attribution(binary: Path, path: Path, cold_workspace: Path, answer_key: Path,
                         cold: dict[str, str], dump_digest: str,
                         timeout: int, scratch: Path, limits: dict[str, Any], label: str) -> str:
    output = run_bounded([str(binary), "--validate-report", str(path), str(cold_workspace), str(answer_key), cold["normalized_rom_sha256"],
                          cold["cold_manifest_sha256"], cold["candidate_identity_v3_sha256"], dump_digest],
                         timeout, label, scratch, limits)
    if len(output.encode()) > 256:
        raise FoldError(f"{label} failed: invalid_summary")
    try:
        summary = require_object(json.loads(output), "attribution validation summary")
        exact_keys(summary, {"schema", "schema_version", "report_sha256"}, set(), "attribution validation summary")
        report_digest = require_digest(summary["report_sha256"], "validated attribution report digest")
    except (FoldError, json.JSONDecodeError) as error:
        raise FoldError(f"{label} failed: invalid_summary") from error
    expected_summary = f'{{"schema":"{VALIDATION_SUMMARY_SCHEMA}","schema_version":1,"report_sha256":"{report_digest}"}}\n'
    if summary["schema"] != VALIDATION_SUMMARY_SCHEMA or summary["schema_version"] != 1 or output != expected_summary:
        raise FoldError(f"{label} failed: invalid_summary")
    actual_digest = sha256_stable_file(path, "validated attribution report", max_bytes=MAX_ATTRIBUTION_BYTES)
    if actual_digest != report_digest:
        raise FoldError(f"{label} failed: report_changed_after_validation")
    return actual_digest


def command_prepare(args: argparse.Namespace) -> None:
    manifest = parse_prepare(canonical_regular(args.manifest, "prepare manifest"))
    limits = subprocess_limits(args.max_rss_mib, args.min_free_percent)
    final = Path(args.run)
    stage = staged_directory(final, "run")
    published = False
    try:
        games, fold, inputs = stage / "games", stage / "fold", stage / "inputs"
        make_private(games); make_private(fold); make_private(inputs)
        bins, executable_identities = stage_binaries(args.bin_dir, stage / "tools")
        game_receipts, events = [], []
        for entry in manifest["entries"]:
            game_root, cold = games / entry["id"], games / entry["id"] / "cold"
            make_private(game_root); make_private(cold)
            staged_rom = inputs / f"{entry['id']}.rom"
            copy_stable_input(entry["rom"], staged_rom, max_bytes=MAX_ROM_BYTES)
            actual_rom = rom_identity(bins["rom_identity"], staged_rom, args.timeout_seconds, stage, limits)
            if actual_rom != entry["rom_digest"]:
                raise FoldError(f"{entry['id']} normalized ROM digest mismatch")
            run_bounded([str(bins["produce_snapshot_workspace"]), "--training", str(staged_rom), str(cold)], args.timeout_seconds, f"{entry['id']} cold producer", stage, limits)
            run_bounded([str(bins["validate_training_workspace"]), str(cold)], args.timeout_seconds, f"{entry['id']} cold validator", stage, limits)
            cold_receipt = cold_identity(cold, actual_rom)
            item: dict[str, Any] = {"id": entry["id"], "family": entry["family"], "role": "held_out" if entry["id"] == manifest["held_out_id"] else "training", "cold": cold_receipt}
            events.append({"event": "cold_validated", "id": entry["id"]})
            if entry["answer"] is not None:
                answer = entry["answer"]
                staged_dump = inputs / f"{entry['id']}.dump.toml"
                actual_dump = copy_stable_input(answer["path"], staged_dump, max_bytes=MAX_DUMP_BYTES)
                if actual_dump != answer["expected_sha256"]:
                    raise FoldError(f"{entry['id']} answer-key digest mismatch")
                grade = game_root / "grade"; make_private(grade)
                events.append({"event": "training_key_admitted", "id": entry["id"]})
                run_bounded([str(bins["attribute_known_functions"]), str(cold), str(staged_dump), actual_rom, actual_dump, str(grade)], args.timeout_seconds, f"{entry['id']} training attribution", stage, limits)
                report_path = grade / "known-function-attribution.json"
                report_digest = validate_attribution(bins["attribute_known_functions"], report_path, cold, staged_dump, cold_receipt,
                                                     actual_dump, args.timeout_seconds, stage, limits,
                                                     f"{entry['id']} training report validator")
                item["training_grade"] = {"answer_key_sha256": actual_dump, "attribution_report_sha256": report_digest, "license_disposition": answer["license_disposition"]}
                events.append({"event": "training_report_validated", "id": entry["id"]})
            game_receipts.append(item)
        receipt = {"schema": SCHEMA_TRAINING, "schema_version": 1, "fold_id": manifest["fold_id"], "training_ids": manifest["training_ids"], "held_out_id": manifest["held_out_id"], "held_out_key_admitted_by_orchestrator": False, "executables": executable_identities, "subprocess_limits": limits, "games": game_receipts, "events": events}
        digest = publish_new(fold / "training-receipt.json", receipt_bytes(receipt))
        publish_directory(stage, final); published = True
    finally:
        if not published:
            shutil.rmtree(stage, ignore_errors=True)
    print(f"cold-training-fold: prepared fold={manifest['fold_id']} games={len(game_receipts)} training={len(manifest['training_ids'])} receipt_sha256={digest}")


def verified_receipt(path: Path, schema: str, label: str) -> tuple[dict[str, Any], str]:
    value, data = read_json(path, label, canonical=True)
    claimed = require_digest(value.get("canonical_sha256"), f"{label} canonical digest")
    body = dict(value); del body["canonical_sha256"]
    if sha256_bytes(canonical_bytes(body)) != claimed:
        raise FoldError(f"{label} canonical digest mismatch")
    if value.get("schema") != schema or value.get("schema_version") != 1:
        raise FoldError(f"unsupported {label} schema")
    return value, sha256_bytes(data)


def validate_executables(value: Any, label: str) -> dict[str, str]:
    value = require_object(value, label)
    exact_keys(value, set(BINARIES), set(), label)
    return {name: require_digest(value[name], f"{label} {name}") for name in BINARIES}


def validate_cold(value: Any, label: str) -> dict[str, str]:
    value = require_object(value, label)
    exact_keys(value, {"normalized_rom_sha256", "cold_manifest_sha256", "candidate_identity_v3_sha256"}, set(), label)
    return {key: require_digest(value[key], f"{label} {key}") for key in value}


def validate_training_receipt(value: dict[str, Any]) -> None:
    exact_keys(value, {"schema", "schema_version", "fold_id", "training_ids", "held_out_id", "held_out_key_admitted_by_orchestrator", "executables", "subprocess_limits", "games", "events", "canonical_sha256"}, set(), "training receipt")
    require_id(value["fold_id"], "training receipt fold_id")
    held = require_id(value["held_out_id"], "training receipt held_out_id")
    if value["held_out_key_admitted_by_orchestrator"] is not False:
        raise FoldError("training receipt says the orchestrator admitted a held-out key")
    validate_executables(value["executables"], "training executables")
    validate_subprocess_limits(value["subprocess_limits"], "training subprocess limits")
    if not isinstance(value["training_ids"], list):
        raise FoldError("training_ids must be an array")
    training = [require_id(item, "training id") for item in value["training_ids"]]
    if not training or training != sorted(set(training)) or held in training:
        raise FoldError("training receipt training_ids relation is invalid")
    if not isinstance(value["games"], list) or not isinstance(value["events"], list):
        raise FoldError("training receipt arrays are malformed")
    expected_ids = sorted([*training, held])
    if [require_object(item, "training game receipt").get("id") for item in value["games"]] != expected_ids:
        raise FoldError("training receipt game ids are not the exact fold")
    expected_events = []
    for item in value["games"]:
        exact_keys(item, {"id", "family", "role", "cold"}, {"training_grade"}, "training game receipt")
        game_id = require_id(item["id"], "training game id"); require_id(item["family"], "training game family")
        role = "held_out" if game_id == held else "training"
        if item["role"] != role or ("training_grade" in item) != (role == "training"):
            raise FoldError("training game role/grade relation is invalid")
        validate_cold(item["cold"], "training cold receipt")
        expected_events.append({"event": "cold_validated", "id": game_id})
        if role == "training":
            grade = require_object(item["training_grade"], "training grade receipt")
            exact_keys(grade, {"answer_key_sha256", "attribution_report_sha256", "license_disposition"}, set(), "training grade receipt")
            require_digest(grade["answer_key_sha256"], "training answer digest"); require_digest(grade["attribution_report_sha256"], "training report digest"); require_id(grade["license_disposition"], "license disposition")
            expected_events.extend(({"event": "training_key_admitted", "id": game_id}, {"event": "training_report_validated", "id": game_id}))
    if value["events"] != expected_events:
        raise FoldError("training receipt event sequence is invalid")


def validate_freeze_receipt(value: dict[str, Any]) -> None:
    exact_keys(value, {"schema", "schema_version", "fold_id", "training_ids", "held_out_id", "training_receipt_sha256", "mechanism_sha256", "held_out_cold", "held_out_key_admitted_by_orchestrator", "executables", "subprocess_limits", "events", "canonical_sha256"}, set(), "freeze receipt")
    require_id(value["fold_id"], "freeze fold_id"); held = require_id(value["held_out_id"], "freeze held_out_id")
    if not isinstance(value["training_ids"], list):
        raise FoldError("freeze training_ids must be an array")
    training = [require_id(item, "freeze training id") for item in value["training_ids"]]
    if not training or training != sorted(set(training)) or held in training:
        raise FoldError("freeze training_ids relation is invalid")
    require_digest(value["training_receipt_sha256"], "freeze training digest"); require_digest(value["mechanism_sha256"], "freeze mechanism digest")
    validate_cold(value["held_out_cold"], "freeze held-out cold receipt"); validate_executables(value["executables"], "freeze executables")
    validate_subprocess_limits(value["subprocess_limits"], "freeze subprocess limits")
    if value["held_out_key_admitted_by_orchestrator"] is not False or value["events"] != [{"event": "training_receipt_validated"}, {"event": "mechanism_frozen"}]:
        raise FoldError("freeze admission claim or event sequence is invalid")


def command_freeze(args: argparse.Namespace) -> None:
    run = private_run(args.run)
    training, training_digest = verified_receipt(run / "fold" / "training-receipt.json", SCHEMA_TRAINING, "training receipt")
    validate_training_receipt(training)
    if training_digest != require_digest(args.expected_training_receipt_sha256, "expected training receipt digest"):
        raise FoldError("training receipt digest mismatch")
    mechanism, mechanism_bytes = read_json(canonical_regular(args.mechanism, "mechanism"), "mechanism", canonical=True)
    exact_keys(mechanism, {"schema", "schema_version", "algorithm", "source_revision_or_patch_digest", "parameter_digest", "training_receipt_sha256", "training_ids", "held_out_id"}, set(), "mechanism")
    if mechanism["schema"] != SCHEMA_MECHANISM or mechanism["schema_version"] != 1:
        raise FoldError("unsupported mechanism schema")
    require_string(mechanism["algorithm"], "mechanism algorithm"); require_digest(mechanism["source_revision_or_patch_digest"], "mechanism source digest"); require_digest(mechanism["parameter_digest"], "mechanism parameter digest")
    if mechanism["training_receipt_sha256"] != training_digest or mechanism["training_ids"] != training["training_ids"] or mechanism["held_out_id"] != training["held_out_id"]:
        raise FoldError("mechanism is not bound to this training fold")
    held = [item for item in training["games"] if item["id"] == training["held_out_id"]]
    if len(held) != 1:
        raise FoldError("training receipt has no unique held-out game")
    freeze = {"schema": SCHEMA_FREEZE, "schema_version": 1, "fold_id": training["fold_id"], "training_ids": training["training_ids"], "held_out_id": training["held_out_id"], "training_receipt_sha256": training_digest, "mechanism_sha256": sha256_bytes(mechanism_bytes), "held_out_cold": held[0]["cold"], "held_out_key_admitted_by_orchestrator": False, "executables": training["executables"], "subprocess_limits": training["subprocess_limits"], "events": [{"event": "training_receipt_validated"}, {"event": "mechanism_frozen"}]}
    digest = publish_new(run / "fold" / "freeze.json", receipt_bytes(freeze))
    print(f"cold-training-fold: frozen fold={training['fold_id']} freeze_sha256={digest}")


def parse_admission(path: Path) -> dict[str, Any]:
    admission, _ = read_json(path, "held-out admission")
    exact_keys(admission, {"schema", "schema_version", "fold_id", "held_out_id", "dump_path", "expected_dump_sha256", "expected_rom_sha256", "expected_freeze_sha256"}, set(), "held-out admission")
    if admission["schema"] != SCHEMA_ADMISSION or admission["schema_version"] != 1:
        raise FoldError("unsupported held-out admission schema")
    return {"fold_id": require_id(admission["fold_id"], "admission fold_id"), "held_out_id": require_id(admission["held_out_id"], "admission held_out_id"), "dump": canonical_regular(admission["dump_path"], "held-out answer key", max_bytes=MAX_DUMP_BYTES), "dump_digest": require_digest(admission["expected_dump_sha256"], "expected held-out dump digest"), "rom_digest": require_digest(admission["expected_rom_sha256"], "expected held-out ROM digest"), "freeze_digest": require_digest(admission["expected_freeze_sha256"], "admission freeze digest")}


def command_grade(args: argparse.Namespace) -> None:
    run = private_run(args.run)
    freeze, freeze_digest = verified_receipt(run / "fold" / "freeze.json", SCHEMA_FREEZE, "freeze receipt")
    validate_freeze_receipt(freeze)
    limits = subprocess_limits(args.max_rss_mib, args.min_free_percent)
    if limits != freeze["subprocess_limits"]:
        raise FoldError("grading subprocess limits differ from the frozen fold")
    if freeze_digest != require_digest(args.expected_freeze_sha256, "expected freeze digest"):
        raise FoldError("freeze digest mismatch")
    admission = parse_admission(canonical_regular(args.admission, "held-out admission"))
    if admission["freeze_digest"] != freeze_digest or admission["fold_id"] != freeze["fold_id"] or admission["held_out_id"] != freeze["held_out_id"]:
        raise FoldError("held-out admission is not bound to this freeze")
    cold = freeze["held_out_cold"]
    if admission["rom_digest"] != cold["normalized_rom_sha256"]:
        raise FoldError("held-out admission ROM digest mismatch")
    final = run / "heldout"
    stage = staged_directory(final, "held-out result")
    published = False
    try:
        bins, identities = stage_binaries(args.bin_dir, stage / "tools")
        if identities != freeze["executables"]:
            raise FoldError("grading executable identities differ from the frozen fold")
        staged_dump = stage / "answer-key.toml"
        actual_dump = copy_stable_input(admission["dump"], staged_dump, max_bytes=MAX_DUMP_BYTES)
        if actual_dump != admission["dump_digest"]:
            raise FoldError("held-out answer-key digest mismatch")
        cold_path = run / "games" / freeze["held_out_id"] / "cold"
        run_bounded([str(bins["validate_training_workspace"]), str(cold_path)], args.timeout_seconds, "held-out cold validator", stage, limits)
        if cold_identity(cold_path, admission["rom_digest"]) != cold:
            raise FoldError("held-out cold workspace changed after freeze")
        grade = stage / "grade"; make_private(grade)
        run_bounded([str(bins["attribute_known_functions"]), str(cold_path), str(staged_dump), admission["rom_digest"], actual_dump, str(grade)], args.timeout_seconds, "held-out attribution", stage, limits)
        report_path = grade / "known-function-attribution.json"
        report_digest = validate_attribution(bins["attribute_known_functions"], report_path, cold_path, staged_dump, cold, actual_dump,
                                             args.timeout_seconds, stage, limits, "held-out report validator")
        heldout = {"schema": SCHEMA_HELDOUT, "schema_version": 1, "fold_id": freeze["fold_id"], "held_out_id": freeze["held_out_id"], "freeze_sha256": freeze_digest, "held_out_cold": cold, "answer_key_sha256": actual_dump, "attribution_report_sha256": report_digest, "executables": identities, "subprocess_limits": limits, "events": [{"event": "freeze_validated"}, {"event": "held_out_key_admitted_by_orchestrator"}, {"event": "held_out_report_validated"}]}
        digest = publish_new(stage / "held-out-receipt.json", receipt_bytes(heldout))
        publish_directory(stage, final); published = True
    finally:
        if not published:
            shutil.rmtree(stage, ignore_errors=True)
    print(f"cold-training-fold: graded-heldout fold={freeze['fold_id']} held_out={freeze['held_out_id']} receipt_sha256={digest}")


def parser() -> argparse.ArgumentParser:
    root = argparse.ArgumentParser(description=__doc__); commands = root.add_subparsers(dest="command", required=True)
    prepare = commands.add_parser("prepare")
    prepare.add_argument("--manifest", required=True); prepare.add_argument("--run", required=True); prepare.add_argument("--bin-dir", required=True); prepare.add_argument("--timeout-seconds", type=int, default=600); prepare.add_argument("--max-rss-mib", type=int, default=DEFAULT_MAX_RSS_MIB); prepare.add_argument("--min-free-percent", type=int, default=DEFAULT_MIN_FREE_PERCENT); prepare.set_defaults(function=command_prepare)
    freeze = commands.add_parser("freeze")
    freeze.add_argument("--run", required=True); freeze.add_argument("--mechanism", required=True); freeze.add_argument("--expected-training-receipt-sha256", required=True); freeze.set_defaults(function=command_freeze)
    grade = commands.add_parser("grade-heldout")
    grade.add_argument("--run", required=True); grade.add_argument("--admission", required=True); grade.add_argument("--expected-freeze-sha256", required=True); grade.add_argument("--bin-dir", required=True); grade.add_argument("--timeout-seconds", type=int, default=600); grade.add_argument("--max-rss-mib", type=int, default=DEFAULT_MAX_RSS_MIB); grade.add_argument("--min-free-percent", type=int, default=DEFAULT_MIN_FREE_PERCENT); grade.set_defaults(function=command_grade)
    return root


def main() -> int:
    try:
        args = parser().parse_args(); args.function(args); return 0
    except FoldError as error:
        print(f"cold-training-fold: {error}", file=sys.stderr); return 1
    except OSError as error:
        print(f"cold-training-fold: operating-system error ({error.errno})", file=sys.stderr); return 1


if __name__ == "__main__":
    raise SystemExit(main())
