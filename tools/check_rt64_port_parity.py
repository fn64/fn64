#!/usr/bin/env python3
"""Validate the exact fail-closed RT64-to-Rust conformance denominator."""

from __future__ import annotations

import argparse
import copy
import hashlib
import json
import os
import secrets
import stat
import subprocess
import sys
import tempfile
import time
from collections import Counter
from dataclasses import dataclass
from pathlib import Path


SCHEMA = "fn64.rt64-port-parity.v2"
FIXTURE_SCHEMA = "fn64.render-conformance.replay.v1"
RECEIPT_SCHEMA = "fn64.render-conformance.receipt.v5"
RUN_SERIES_SCHEMA = "fn64.render-conformance.run-series.v4"
PROCESS_RESULT_SCHEMA = "fn64.render-conformance.process-result.v3"
RUN_REQUEST_SCHEMA = "fn64.render-conformance.runner-request.v2"
REQUIRED_RUNS = 10
MAX_ARTIFACT_BYTES = 64 * 1024 * 1024
MAX_PROCESS_OUTPUT_BYTES = 4 * 1024 * 1024
RUNNER_TIMEOUT_SECONDS = 30
OBSERVABLES = [
    "admitted_commands_state", "full_sync_timeline", "tmem_bytes",
    "resource_journal_guest_memory_effects", "shader_parameters",
    "framebuffer_native", "framebuffer_high", "vi", "post_vi_pixels",
]
STATE_VALUES = {
    "RT64_PASS", "RT64_DIVERGES", "RT64_PUBLICLY_UNAVAILABLE",
    "RUST_PENDING", "RUST_PASS", "RUST_BOUNDED_QUALIFICATION",
}
RT64_STATES = {"RT64_PASS", "RT64_DIVERGES", "RT64_PUBLICLY_UNAVAILABLE"}
RUST_STATES = {"RUST_PENDING", "RUST_PASS", "RUST_BOUNDED_QUALIFICATION"}
AUTHORITIES = {"hardware_reference", "admitted_full_rom", "base_renderer_matrix", "pinned_rt64"}
AVAILABILITY = {"qualified", "unexercised", "build_not_enabled", "platform_unavailable"}
CONTRACT_DIGEST = "7556031949f3093d616f75724e5be091beda5152140c489127213917ef382da0"
STATE_DIGEST = "361f5e59e61f85ead5620b9aeb695db6bdc0f986c72d91f9fe0c5fcc7da6671d"


class ParityError(Exception):
    pass


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ParityError(message)


def canonical_bytes(value: object) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()


def canonical_digest(value: object) -> str:
    return hashlib.sha256(canonical_bytes(value)).hexdigest()


def framed_digest(domain: bytes, fields: list[bytes]) -> str:
    digest = hashlib.sha256()
    digest.update(domain)
    for field in fields:
        digest.update(len(field).to_bytes(8, "big"))
        digest.update(field)
    return digest.hexdigest()


def is_digest(value: object) -> bool:
    return isinstance(value, str) and len(value) == 64 and all(c in "0123456789abcdef" for c in value)


def load_json(path: Path) -> dict:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ParityError(f"cannot read {path}: {error}") from error
    require(isinstance(value, dict), f"{path}: root must be an object")
    return value


@dataclass(frozen=True)
class RetainedArtifact:
    path: str
    sha256: str
    object_identity: tuple[int, int]
    bytes: bytes


class ArtifactRegistry:
    """Stable regular-file loader with lexical, symlink, and alias rejection."""

    def __init__(self, root: Path):
        self.root = root
        self._objects: dict[tuple[int, int], str] = {}
        self._paths: set[str] = set()
        self._artifacts: list[RetainedArtifact] = []

    def load(self, reference: object, *, prefix: tuple[str, ...]) -> RetainedArtifact:
        require(isinstance(reference, dict) and set(reference) == {"path", "sha256"}, "artifact reference fields drifted")
        relative = reference["path"]
        expected = reference["sha256"]
        require(isinstance(relative, str) and relative, "artifact path must be a nonempty string")
        require(is_digest(expected), f"{relative}: artifact SHA-256 must be lowercase hex")
        path = Path(relative)
        require(not path.is_absolute(), f"{relative}: artifact path must be repository-relative")
        require(path.parts and all(part not in {"", ".", ".."} for part in path.parts), f"{relative}: artifact path is not normalized")
        require(path.parts[:len(prefix)] == prefix, f"{relative}: artifact path is outside {'/'.join(prefix)}")
        require(relative not in self._paths, f"{relative}: artifact path is reused")

        current = self.root
        try:
            for part in path.parts:
                current = current / part
                link_state = os.lstat(current)
                require(not stat.S_ISLNK(link_state.st_mode), f"{relative}: symlinked artifact path is forbidden")
            flags = os.O_RDONLY
            if hasattr(os, "O_CLOEXEC"):
                flags |= os.O_CLOEXEC
            if hasattr(os, "O_NOFOLLOW"):
                flags |= os.O_NOFOLLOW
            descriptor = os.open(self.root / path, flags)
        except OSError as error:
            raise ParityError(f"{relative}: cannot open retained artifact: {error}") from error

        try:
            before = os.fstat(descriptor)
            require(stat.S_ISREG(before.st_mode), f"{relative}: retained artifact must be a regular file")
            require(before.st_size <= MAX_ARTIFACT_BYTES, f"{relative}: retained artifact exceeds {MAX_ARTIFACT_BYTES} bytes")
            chunks: list[bytes] = []
            digest = hashlib.sha256()
            while True:
                chunk = os.read(descriptor, 1024 * 1024)
                if not chunk:
                    break
                digest.update(chunk)
                chunks.append(chunk)
            after = os.fstat(descriptor)
        finally:
            os.close(descriptor)

        try:
            path_after = os.lstat(self.root / path)
        except OSError as error:
            raise ParityError(f"{relative}: retained artifact disappeared after read: {error}") from error
        stable = (
            before.st_dev, before.st_ino, before.st_mode, before.st_size, before.st_mtime_ns,
        ) == (
            after.st_dev, after.st_ino, after.st_mode, after.st_size, after.st_mtime_ns,
        ) == (
            path_after.st_dev, path_after.st_ino, path_after.st_mode,
            path_after.st_size, path_after.st_mtime_ns,
        )
        require(stable, f"{relative}: retained artifact changed identity or bytes during verification")
        object_identity = (before.st_dev, before.st_ino)
        require(object_identity not in self._objects, f"{relative}: aliases retained artifact {self._objects.get(object_identity)}")
        actual = digest.hexdigest()
        require(actual == expected, f"{relative}: artifact SHA-256 mismatch")
        self._paths.add(relative)
        self._objects[object_identity] = relative
        artifact = RetainedArtifact(relative, actual, object_identity, b"".join(chunks))
        self._artifacts.append(artifact)
        return artifact

    def assert_all_unchanged(self) -> None:
        for artifact in self._artifacts:
            path = self.root / artifact.path
            try:
                state = os.lstat(path)
                require(stat.S_ISREG(state.st_mode) and not stat.S_ISLNK(state.st_mode), f"{artifact.path}: retained artifact ceased to be a regular file")
                require((state.st_dev, state.st_ino) == artifact.object_identity, f"{artifact.path}: retained artifact identity changed after verification")
                require(hashlib.sha256(path.read_bytes()).hexdigest() == artifact.sha256, f"{artifact.path}: retained artifact changed after verification")
            except OSError as error:
                raise ParityError(f"{artifact.path}: retained artifact unavailable after verification: {error}") from error


@dataclass(frozen=True)
class RunnerPolicy:
    delegate_kind: str
    runner_path: str
    runner_sha256: str
    build_receipt_path: str
    build_receipt_sha256: str
    verifier_path: str
    verifier_sha256: str
    authority_path: str
    authority_sha256: str
    runner_args: tuple[str, ...] = ("run", "honest")
    test_only: bool = False


# Fail closed. A future backend ticket must add a reviewed concrete runner and
# its exact retained executable identity in the same change as its verifier.
REGISTERED_RUNNERS: dict[str, RunnerPolicy] = {}


@dataclass(frozen=True)
class ExecutedSeries:
    semantic_identity: str
    process_identities: tuple[str, ...]
    run_identities: tuple[str, ...]
    series_identity: str


def source_denominator(root: Path) -> tuple[list[tuple[str, str]], dict[tuple[str, str], dict]]:
    base = load_json(root / "docs/base-renderer-behavior-matrix.json")
    features = load_json(root / "docs/rt64-public-feature-inventory.json")
    base_rows = [("base_renderer", item["id"]) for item in base.get("items", [])]
    feature_rows = [
        ("rt64_public_feature", item["id"])
        for item in features.get("items", []) if item.get("scope") == "behavior"
    ]
    require(len(base_rows) == 24, "base-renderer source denominator must contain exactly 24 rows")
    require(len(feature_rows) == 26, "RT64 public-behavior source denominator must contain exactly 26 rows")
    metadata = {("base_renderer", item["id"]): item for item in base["items"]}
    metadata.update({
        ("rt64_public_feature", item["id"]): item
        for item in features["items"] if item.get("scope") == "behavior"
    })
    return base_rows + feature_rows, metadata


def artifact_reference(path: str, digest: str) -> dict:
    return {"path": path, "sha256": digest}


def parse_json_artifact(artifact: RetainedArtifact) -> dict:
    try:
        value = json.loads(artifact.bytes)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ParityError(f"{artifact.path}: invalid retained JSON: {error}") from error
    require(isinstance(value, dict), f"{artifact.path}: retained JSON root must be an object")
    return value


def optional_digest_fields(value: object, field: str) -> tuple[bytes, bytes]:
    if value is None:
        return b"\0", bytes(32)
    require(is_digest(value), f"{field} must be a SHA-256 digest or null")
    return b"\1", bytes.fromhex(value)


def rt64_source_identity(root: Path) -> str:
    source_id = load_json(root / "docs/rt64-port-authority.json")["oracle"]["source_id"]
    require(isinstance(source_id, str) and source_id.startswith("git:"), "gated RT64 oracle source ID is invalid")
    return framed_digest(
        b"fn64.render-conformance.rt64-source.v1\0",
        [source_id.encode()],
    )


def hex_bytes(value: object, field: str, maximum: int = MAX_ARTIFACT_BYTES) -> bytes:
    require(isinstance(value, str) and len(value) % 2 == 0, f"{field} must be even-length lowercase hex")
    require(all(character in "0123456789abcdef" for character in value), f"{field} must be lowercase hex")
    result = bytes.fromhex(value)
    require(len(result) <= maximum, f"{field} exceeds {maximum} bytes")
    return result


def invoke_verifier(executable: Path, arguments: tuple[str, ...], value: dict, row_id: str) -> dict:
    try:
        completed = subprocess.run(
            [str(executable), *arguments], input=canonical_bytes(value), stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, timeout=RUNNER_TIMEOUT_SECONDS, check=False,
        )
    except (OSError, subprocess.SubprocessError) as error:
        raise ParityError(f"{row_id}: Rust verifier launch failed: {error}") from error
    require(completed.returncode == 0, f"{row_id}: Rust verifier rejected evidence: {completed.stderr.decode(errors='replace')[:1000]}")
    try:
        result = json.loads(completed.stdout)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ParityError(f"{row_id}: Rust verifier emitted invalid JSON: {error}") from error
    require(isinstance(result, dict), f"{row_id}: Rust verifier output must be an object")
    return result


def validate_build_receipt(
    root: Path, registry: ArtifactRegistry, receipt_artifact: RetainedArtifact,
    runner: RetainedArtifact, policy: RunnerPolicy, delegate_kind: str,
) -> tuple[dict, str | None]:
    receipt = parse_json_artifact(receipt_artifact)
    require(set(receipt) == {"schema", "runner", "source_inputs", "build_inputs", "toolchain", "rt64_source_identity", "closure_identity"}, "build receipt fields drifted")
    require(receipt["schema"] == "fn64.render-conformance.build-receipt.v1", "wrong build receipt schema")
    require(receipt["runner"] == artifact_reference(runner.path, runner.sha256), "build receipt does not identify the executed artifact")
    for field in ("source_inputs", "build_inputs"):
        references = receipt[field]
        require(isinstance(references, list) and references, f"build receipt {field} must be nonempty")
        for reference in references:
            registry.load(reference, prefix=("evidence", "rt64-port", "artifacts"))
    includes_synthetic_runner = any(
        reference.get("path", "").endswith("fn64-render-conformance-test-runner.rs")
        for reference in receipt["source_inputs"]
    )
    require(
        includes_synthetic_runner == policy.test_only,
        "synthetic conformance runner source is forbidden in production build receipts",
    )
    toolchain = receipt["toolchain"]
    require(isinstance(toolchain, dict) and set(toolchain) == {"rustc_vv"}, "build receipt toolchain fields drifted")
    try:
        actual_rustc = subprocess.run(["rustc", "-vV"], text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=True).stdout
    except (OSError, subprocess.SubprocessError) as error:
        raise ParityError(f"cannot verify build toolchain: {error}") from error
    require(toolchain["rustc_vv"] == actual_rustc, "build receipt toolchain does not match verifier host")
    source_identity = receipt["rt64_source_identity"]
    if delegate_kind == "rt64":
        require(source_identity == rt64_source_identity(root), "build receipt RT64 source pin mismatch")
    else:
        require(source_identity is None, "non-RT64 build receipt invented an RT64 source pin")
    closure = {
        "runner_sha256": runner.sha256, "source_inputs": receipt["source_inputs"],
        "build_inputs": receipt["build_inputs"], "toolchain": toolchain,
        "rt64_source_identity": source_identity,
    }
    require(receipt["closure_identity"] == canonical_digest(closure), "build receipt closure identity mismatch")
    require(receipt_artifact.path == policy.build_receipt_path and receipt_artifact.sha256 == policy.build_receipt_sha256, "build receipt is not the registered receipt")
    return receipt, source_identity


def _execute_qualified(
    root: Path,
    evidence: dict,
    row: dict,
    delegate_kind: str,
    expected_outcome: str,
    *,
    runner_copy_directory: Path,
    runner_registry: dict[str, RunnerPolicy] | None = None,
) -> ExecutedSeries:
    require(set(evidence) == {"availability", "execution"}, f"{row['id']}: closed evidence fields drifted")
    require(evidence["availability"] == "qualified", f"{row['id']}: closed evidence must be qualified")
    execution = evidence["execution"]
    require(isinstance(execution, dict) and set(execution) == {"runner_id", "runner_artifact", "verifier_artifact", "build_receipt", "replay_artifact", "authority_artifact"}, f"{row['id']}: execution request fields drifted")
    production_registry = runner_registry is None
    policies = REGISTERED_RUNNERS if production_registry else runner_registry
    runner_id = execution["runner_id"]
    require(isinstance(runner_id, str) and runner_id in policies, f"{row['id']}: no reviewed concrete {delegate_kind} runner is registered")
    policy = policies[runner_id]
    require(
        policy.test_only is not production_registry,
        f"{row['id']}: synthetic test runner cannot enter the production registry",
    )
    require(policy.delegate_kind == delegate_kind, f"{row['id']}: runner delegate policy mismatch")

    registry = ArtifactRegistry(root)
    runner = registry.load(execution["runner_artifact"], prefix=("evidence", "rt64-port", "artifacts"))
    verifier = registry.load(execution["verifier_artifact"], prefix=("evidence", "rt64-port", "artifacts"))
    build_receipt = registry.load(execution["build_receipt"], prefix=("evidence", "rt64-port", "artifacts"))
    replay_artifact = registry.load(execution["replay_artifact"], prefix=("evidence", "rt64-port", "artifacts"))
    authority_artifact = registry.load(execution["authority_artifact"], prefix=("evidence", "rt64-port", "artifacts"))
    require(runner.path == policy.runner_path and runner.sha256 == policy.runner_sha256, f"{row['id']}: runner artifact is not the registered executable")
    require(verifier.path == policy.verifier_path and verifier.sha256 == policy.verifier_sha256, f"{row['id']}: Rust verifier artifact is not registered")
    require(authority_artifact.path == policy.authority_path and authority_artifact.sha256 == policy.authority_sha256, f"{row['id']}: verifier-private authority artifact is not registered")
    retained_runner_path = root / runner.path
    require(os.access(retained_runner_path, os.X_OK), f"{row['id']}: registered runner artifact is not executable")
    runner_path = runner_copy_directory / "runner"
    runner_path.write_bytes(runner.bytes)
    runner_path.chmod(0o700)
    verifier_path = runner_copy_directory / "verifier"
    verifier_path.write_bytes(verifier.bytes)
    verifier_path.chmod(0o700)
    _, source_identity = validate_build_receipt(root, registry, build_receipt, runner, policy, delegate_kind)
    replay = parse_json_artifact(replay_artifact)
    authority = parse_json_artifact(authority_artifact)
    inspection = invoke_verifier(verifier_path, ("inspect",), replay, row["id"])
    require(inspection.get("row_id") == row["id"], f"{row['id']}: replay row mismatch")
    require(inspection.get("capture_layer") == row["earliest_observable"], f"{row['id']}: replay does not target frozen earliest observable")
    guest_required = "resource_journal_guest_memory_effects" in row["observable_layers"]
    slots = inspection.get("effect_slots")
    require(isinstance(slots, list), f"{row['id']}: Rust inspection omitted effect slots")
    require(any(slot.get("guest_visible") is True for slot in slots) == guest_required, f"{row['id']}: replay guest effects disagree with row contract")

    semantic_identities: list[str] = []
    process_identities: list[str] = []
    run_identities: list[str] = []
    challenges: set[str] = set()
    for ordinal in range(REQUIRED_RUNS):
        challenge = secrets.token_hex(32)
        require(challenge not in challenges, f"{row['id']}: verifier challenge collision")
        challenges.add(challenge)
        request = {
            "schema": RUN_REQUEST_SCHEMA,
            "ordinal": ordinal,
            "challenge": challenge,
            "replay": replay,
        }
        launch_start_ns = time.monotonic_ns()
        try:
            process = subprocess.Popen(
                [str(runner_path), *policy.runner_args], cwd=runner_copy_directory, stdin=subprocess.PIPE,
                stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            )
            stdout, stderr = process.communicate(
                canonical_bytes(request) + b"\n", timeout=RUNNER_TIMEOUT_SECONDS,
            )
        except subprocess.TimeoutExpired as error:
            process.kill()
            process.communicate()
            raise ParityError(f"{row['id']}: runner exceeded {RUNNER_TIMEOUT_SECONDS}s") from error
        except (OSError, subprocess.SubprocessError) as error:
            raise ParityError(f"{row['id']}: runner launch failed: {error}") from error
        require(process.returncode == 0, f"{row['id']}: runner exited {process.returncode}: {stderr.decode(errors='replace')[:1000]}")
        require(len(stdout) <= MAX_PROCESS_OUTPUT_BYTES, f"{row['id']}: runner output exceeds {MAX_PROCESS_OUTPUT_BYTES} bytes")
        try:
            result = json.loads(stdout)
        except (UnicodeDecodeError, json.JSONDecodeError) as error:
            raise ParityError(f"{row['id']}: runner emitted invalid JSON: {error}") from error
        require(isinstance(result, dict), f"{row['id']}: runner output must be an object")
        require(result.get("schema") == PROCESS_RESULT_SCHEMA, f"{row['id']}: wrong process-result schema")
        require(result.get("challenge") == challenge, f"{row['id']}: runner did not answer this verifier challenge")
        require(result.get("pid") == process.pid, f"{row['id']}: runner result is not from the launched child PID")
        evaluation = invoke_verifier(verifier_path, ("evaluate",), {
            "replay": replay, "authority": authority, "result": result,
        }, row["id"])
        registry.assert_all_unchanged()
        required_classification = "diverges" if expected_outcome == "RT64_DIVERGES" else "pass"
        require(evaluation.get("classification") == required_classification, f"{row['id']}: backend result does not satisfy {expected_outcome}")
        semantic = framed_digest(b"fn64.render-conformance.receipt.v5\0", [
            row["id"].encode(), bytes.fromhex(evaluation["semantic_identity"]),
            bytes.fromhex(runner.sha256), bytes.fromhex(build_receipt.sha256),
            (source_identity or "").encode(), delegate_kind.encode(), expected_outcome.encode(),
        ])
        process_identity = framed_digest(
            b"fn64.render-conformance.fresh-process.v3\0",
            [
                bytes.fromhex(runner.sha256), bytes.fromhex(build_receipt.sha256),
                bytes.fromhex(replay_artifact.sha256), bytes.fromhex(authority_artifact.sha256), bytes.fromhex(challenge),
                process.pid.to_bytes(8, "big", signed=False),
                launch_start_ns.to_bytes(16, "big", signed=False),
            ],
        )
        run_identity = framed_digest(
            b"fn64.render-conformance.process-run.v4\0",
            [bytes.fromhex(semantic), bytes.fromhex(process_identity), hashlib.sha256(stdout).digest()],
        )
        semantic_identities.append(semantic)
        process_identities.append(process_identity)
        run_identities.append(run_identity)
    require(len(set(semantic_identities)) == 1, f"{row['id']}: backend semantic result changed across fresh processes")
    require(len(set(process_identities)) == REQUIRED_RUNS, f"{row['id']}: fresh-process identities were reused")
    require(len(set(run_identities)) == REQUIRED_RUNS, f"{row['id']}: process results were cloned")
    semantic = semantic_identities[0]
    series = framed_digest(
        b"fn64.render-conformance.run-series.v4\0",
        [item for run in run_identities for item in (bytes.fromhex(run), bytes.fromhex(semantic))],
    )
    return ExecutedSeries(semantic, tuple(process_identities), tuple(run_identities), series)


def execute_qualified(
    root: Path,
    evidence: dict,
    row: dict,
    delegate_kind: str,
    expected_outcome: str,
    *,
    runner_registry: dict[str, RunnerPolicy] | None = None,
) -> ExecutedSeries:
    with tempfile.TemporaryDirectory(prefix="fn64-conformance-runner-") as temporary:
        return _execute_qualified(
            root,
            evidence,
            row,
            delegate_kind,
            expected_outcome,
            runner_copy_directory=Path(temporary),
            runner_registry=runner_registry,
        )


def validate_rt64_evidence(root: Path, row: dict, state: str | None) -> None:
    evidence = row["rt64_evidence"]
    require(isinstance(evidence, dict), f"{row['id']}.rt64_evidence must be an object")
    require(evidence.get("availability") in AVAILABILITY, f"{row['id']}: invalid RT64 availability")
    if state is None:
        require(evidence["availability"] in {"unexercised", "build_not_enabled", "platform_unavailable"}, f"{row['id']}: qualified evidence requires a state")
        require(set(evidence) == {"availability", "reason"}, f"{row['id']}: open RT64 evidence fields drifted")
        require(isinstance(evidence["reason"], str) and evidence["reason"].strip(), f"{row['id']}: empty RT64 blocker")
        return
    require(state != "RT64_PUBLICLY_UNAVAILABLE", f"{row['id']}: every denominator row is publicly advertised; public-unavailable is forbidden")
    require(state in {"RT64_PASS", "RT64_DIVERGES"}, f"{row['id']}: invalid RT64 state")
    if state == "RT64_DIVERGES":
        require(row["authority"] != "pinned_rt64", f"{row['id']}: RT64 cannot authorize divergence from itself")
    execute_qualified(root, evidence, row, "rt64", state)


def validate_rust_evidence(root: Path, row: dict, state: str) -> None:
    evidence = row["rust_evidence"]
    require(isinstance(evidence, dict), f"{row['id']}.rust_evidence must be an object")
    if state == "RUST_PENDING":
        require(set(evidence) == {"availability", "reason"}, f"{row['id']}: pending Rust evidence fields drifted")
        require(evidence["availability"] == "unimplemented", f"{row['id']}: pending Rust row must remain explicit")
        require(isinstance(evidence["reason"], str) and evidence["reason"].strip(), f"{row['id']}: empty Rust blocker")
        return
    execute_qualified(root, evidence, row, "rust_port", state)


def validate_manifest(manifest: dict, root: Path) -> tuple[list[dict], dict[tuple[str, str], dict]]:
    require(manifest.get("schema") == SCHEMA, f"schema must be {SCHEMA!r}")
    require(set(manifest) == {"schema", "contract", "rows"}, "manifest root fields drifted")
    contract = manifest["contract"]
    require(contract == {
        "crate": "fn64-render-conformance",
        "fixture_schema": FIXTURE_SCHEMA,
        "receipt_schema": RECEIPT_SCHEMA,
        "run_series_schema": RUN_SERIES_SCHEMA,
        "process_result_schema": PROCESS_RESULT_SCHEMA,
        "run_request_schema": RUN_REQUEST_SCHEMA,
        "observable_order": OBSERVABLES,
        "delegate_kinds": ["rt64", "rust_port", "reference"],
        "rt64_observation_policy": "required_for_every_row",
    }, "conformance contract identity drifted")

    source_rows, metadata = source_denominator(root)
    expected_sources = set(source_rows)
    rows = manifest["rows"]
    require(isinstance(rows, list) and len(rows) == 50, "parity denominator must contain exactly 50 rows")
    seen_ids: set[str] = set()
    seen_sources: set[tuple[str, str]] = set()
    for index, row in enumerate(rows):
        where = f"rows[{index}]"
        require(isinstance(row, dict) and set(row) == {"id", "source", "required", "authority", "observable_layers", "earliest_observable", "states", "rt64_evidence", "rust_evidence"}, f"{where}: unknown or missing fields")
        row_id = row["id"]
        require(isinstance(row_id, str) and row_id and row_id not in seen_ids, f"{where}: invalid or duplicate id")
        seen_ids.add(row_id)
        source = row["source"]
        require(isinstance(source, dict) and set(source) == {"kind", "id"}, f"{row_id}: invalid source")
        source_key = (source["kind"], source["id"])
        prefix = "base" if source["kind"] == "base_renderer" else "feature"
        require(row_id == f"{prefix}::{source['id']}", f"{row_id}: id does not bind exact source")
        require(source_key in expected_sources and source_key not in seen_sources, f"{row_id}: source is outside or duplicates denominator")
        seen_sources.add(source_key)
        require(row["required"] is True, f"{row_id}: denominator rows cannot become optional")
        require(row["authority"] in AUTHORITIES, f"{row_id}: unknown authority")
        layers = row["observable_layers"]
        require(isinstance(layers, list) and layers and len(layers) == len(set(layers)), f"{row_id}: observable layers invalid")
        require(all(layer in OBSERVABLES for layer in layers), f"{row_id}: unknown observable layer")
        require(layers == sorted(layers, key=OBSERVABLES.index), f"{row_id}: observable order drifted")
        require(row["earliest_observable"] == layers[0], f"{row_id}: earliest observable drifted")
        states = row["states"]
        require(isinstance(states, list) and states and len(states) == len(set(states)) and set(states) <= STATE_VALUES, f"{row_id}: state invalid or implicit skip")
        rt64 = [state for state in states if state in RT64_STATES]
        rust = [state for state in states if state in RUST_STATES]
        require(len(rt64) <= 1 and len(rust) == 1, f"{row_id}: exactly one Rust and at most one RT64 state required during development")
        validate_rt64_evidence(root, row, rt64[0] if rt64 else None)
        validate_rust_evidence(root, row, rust[0])
    require(seen_sources == expected_sources, "source denominator drifted")

    frozen = [{"id": row["id"], "authority": row["authority"], "observable_layers": row["observable_layers"], "earliest_observable": row["earliest_observable"]} for row in rows]
    require(canonical_digest(frozen) == CONTRACT_DIGEST, "one or more frozen row authority/observable contracts drifted")
    frozen_states = [{"id": row["id"], "states": row["states"]} for row in rows]
    require(canonical_digest(frozen_states) == STATE_DIGEST, "row state ledger drifted; closure or reopening requires an explicit reviewed ledger update")
    return rows, metadata


def rejection_guards(manifest: dict, root: Path) -> None:
    def rejected(label: str, mutate) -> None:
        candidate = copy.deepcopy(manifest)
        mutate(candidate)
        try:
            validate_manifest(candidate, root)
        except ParityError:
            return
        raise ParityError(f"validator rejection guard failed: {label}")

    rejected("denominator deletion", lambda value: value["rows"].pop())
    rejected("implicit skip", lambda value: value["rows"][0].__setitem__("states", ["SKIPPED"]))
    for row_index in range(len(manifest["rows"])):
        rejected(f"authority shrink row {row_index}", lambda value, i=row_index: value["rows"][i].__setitem__("authority", "hardware_reference" if value["rows"][i]["authority"] != "hardware_reference" else "pinned_rt64"))
        rejected(
            f"observable contract mutation row {row_index}",
            lambda value, i=row_index: value["rows"][i].update(
                observable_layers=["post_vi_pixels" if value["rows"][i]["observable_layers"] != ["post_vi_pixels"] else "admitted_commands_state"],
                earliest_observable="post_vi_pixels" if value["rows"][i]["observable_layers"] != ["post_vi_pixels"] else "admitted_commands_state",
            ),
        )
    rejected("base public-unavailable spoof", lambda value: value["rows"][0].update(states=["RT64_PUBLICLY_UNAVAILABLE", "RUST_PENDING"], rt64_evidence={"availability": "unexercised", "reason": "fake"}))
    feature = next(i for i, row in enumerate(manifest["rows"]) if row["source"]["kind"] == "rt64_public_feature")
    rejected("available feature public-unavailable spoof", lambda value: value["rows"][feature].update(states=["RT64_PUBLICLY_UNAVAILABLE", "RUST_PENDING"], rt64_evidence={"availability": "unexercised", "reason": "fake"}))
    rejected("README needle Rust pass spoof", lambda value: value["rows"][0].update(states=["RUST_PASS"], rust_evidence={"availability": "qualified", "series": artifact_reference("README.md", "0" * 64)}))
    rejected("README needle RT64 pass spoof", lambda value: value["rows"][0].update(states=["RT64_PASS", "RUST_PENDING"], rt64_evidence={"availability": "qualified", "series": artifact_reference("README.md", "0" * 64)}))

    fake_series = {
        "availability": "qualified",
        "execution": {
            "runner_id": "hand-authored",
            "runner_artifact": artifact_reference("evidence/rt64-port/artifacts/runner", "1" * 64),
            "build_artifact": artifact_reference("evidence/rt64-port/artifacts/build", "2" * 64),
            "fixture_artifact": artifact_reference("evidence/rt64-port/artifacts/fixture", "3" * 64),
            "runs": [{"process_token": "4" * 64, "semantic": "5" * 64}] * REQUIRED_RUNS,
        },
    }
    try:
        execute_qualified(root, fake_series, manifest["rows"][0], "rust_port", "RUST_PASS")
    except ParityError as error:
        require("execution request fields drifted" in str(error), "hand-authored JSON failed for the wrong reason")
    else:
        raise ParityError("validator rejection guard failed: hand-authored typed series")


def source_link(source: dict) -> str:
    filename = "base-renderer-behavior-matrix.json" if source["kind"] == "base_renderer" else "rt64-public-feature-inventory.json"
    label = "base renderer" if source["kind"] == "base_renderer" else "RT64 feature"
    return f"[{label}]({filename}) / `{source['id']}`"


def render_doc(rows: list[dict], metadata: dict[tuple[str, str], dict]) -> str:
    counts = Counter(state for row in rows for state in row["states"])
    rust_pending = sum("RUST_PENDING" in row["states"] for row in rows)
    rt64_pending = sum(not any(state in RT64_STATES for state in row["states"]) for row in rows)
    lines = [
        "# RT64-to-Rust renderer parity ladder", "",
        "Generated by `tools/check_rt64_port_parity.py` from",
        "`docs/rt64-port-parity.json`; do not edit this file by hand.", "",
        "The ordinary checker keeps a structurally sound development backlog green. The",
        "separate `--progress` gate remains red until every required row has both a",
        "qualified RT64 observation/classification and a qualified Rust result.", "",
        "No concrete backend runner is registered today. Closed evidence is therefore",
        "fail-closed: the checker itself launches every fresh process and owns each random",
        "challenge. A public replay artifact contains only the Rust-decoded record, exact raw",
        "payload streams, and capture control; a separately registered verifier-private authority",
        "contains expected observations/effects and is never sent to the child.", "", "## Summary", "",
        "| State | Rows |", "|---|---:|",
    ]
    for state in ["RT64_PASS", "RT64_DIVERGES", "RT64_PUBLICLY_UNAVAILABLE", "RUST_PENDING", "RUST_PASS", "RUST_BOUNDED_QUALIFICATION"]:
        lines.append(f"| `{state}` | {counts[state]} |")
    lines.extend([
        "",
        f"Exact denominator: **{len(rows)} required rows** (24 base-renderer + 26 publicly advertised RT64 features); **{rust_pending} Rust rows pending** and **{rt64_pending} RT64 observations pending**.",
        "", "## Rows", "",
        "| Row | Source claim | Authority | Frozen observables | States | RT64 evidence |",
        "|---|---|---|---|---|---|",
    ])
    for row in rows:
        source = row["source"]
        title = metadata[(source["kind"], source["id"])]["title"]
        layers = " → ".join(f"`{layer}`" for layer in row["observable_layers"])
        states = ", ".join(f"`{state}`" for state in row["states"])
        lines.append(f"| `{row['id']}` | {source_link(source)} — {title} | `{row['authority']}` | {layers} | {states} | `{row['rt64_evidence']['availability']}` |")
    lines.extend([
        "", "## Commands", "", "Ordinary structural/evidence gate:", "", "```sh",
        "python3 tools/check_rt64_port_parity.py", "cargo test -p fn64-render-conformance", "```", "",
        "Deliberately-red completion gate:", "", "```sh",
        "python3 tools/check_rt64_port_parity.py --progress", "```", "",
        "## Open concrete-runner frontier", "",
        "No qualified headless RT64 delegate exposes a backend-produced observable for this",
        "contract, and the Rust renderer has only a synthetic lifecycle spine. The previous raw-DPC attempt",
        "reported fn64 preflight FullSync rather than an RT64-produced observation and the",
        "headless host stopped at SDL display initialization. Those facts are blockers, not",
        "passes, divergences, or public-source unavailability.", "",
        "A future runner registration must land with exact retained runner, Rust verifier, private",
        "authority, and build-receipt identities. The build receipt binds the executed artifact to",
        "its source inputs, build inputs, active Rust toolchain, and (for RT64) the gated source pin.",
        "The verifier Rust-decodes every WorkloadRecord, reconstructs exact payload-bound IR, and",
        "derives pass/divergence from backend output. For guest-visible rows, the exact reviewed",
        "runner source/build/binary is the trust root for its internal GuestCommittedTicket",
        "lifecycle; the verifier independently binds the emitted proof to the replay, exact effects,",
        "and fresh challenge. JSON hashing alone is not Rust type provenance.",
        "The checker rejects symlinks, hard-link aliases, post-read mutation, cloned process output,",
        "and any caller-authored authority or run series.", "",
    ])
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifest", type=Path, default=Path("docs/rt64-port-parity.json"))
    parser.add_argument("--doc", type=Path, default=Path("docs/RT64-PORT-PARITY.md"))
    parser.add_argument("--write-doc", action="store_true")
    parser.add_argument("--print-doc", action="store_true")
    parser.add_argument("--progress", action="store_true")
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    try:
        require(not (args.write_doc and args.print_doc), "--write-doc and --print-doc are mutually exclusive")
        manifest = load_json(args.manifest)
        rows, metadata = validate_manifest(manifest, root)
        rejection_guards(manifest, root)
        rendered = render_doc(rows, metadata)
        if args.print_doc:
            sys.stdout.write(rendered)
            return 0
        if args.write_doc:
            args.doc.write_text(rendered, encoding="utf-8")
            print(f"rt64-port-parity: wrote {args.doc}")
            return 0
        require(args.doc.read_text(encoding="utf-8") == rendered, f"generated doc is stale: {args.doc}; regenerate with --write-doc")
        rust_pending = [row["id"] for row in rows if "RUST_PENDING" in row["states"]]
        rt64_pending = [row["id"] for row in rows if not any(state in RT64_STATES for state in row["states"])]
        if args.progress and (rust_pending or rt64_pending):
            print(
                f"rt64-port-parity: port incomplete ({len(rust_pending)}/{len(rows)} Rust rows pending; {len(rt64_pending)}/{len(rows)} RT64 observations pending)",
                file=sys.stderr,
            )
            return 2
        print(f"rt64-port-parity: clean ({len(rows)} required rows; {len(rust_pending)} Rust pending; {len(rt64_pending)} RT64 observations pending)")
        return 0
    except (OSError, ParityError) as error:
        print(f"rt64-port-parity: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
