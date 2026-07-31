#!/usr/bin/env python3
"""Content-silent parsers and fixture helpers for WM prepared-shard gates."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import sys
import tempfile


ROOT_SCHEMA = "fn64.wm-prepared-shard-tree.v2"
ARTIFACT_SCHEMA = "fn64.wm-prepared-shard-artifact.v1"
HEX_RE = re.compile(r"[0-9a-f]{64}")


class AuditError(Exception):
    pass


def fail(message: str) -> None:
    raise AuditError(message)


def digest_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def digest_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def canonical_digest(value: str, label: str) -> str:
    if not HEX_RE.fullmatch(value) or value == "0" * 64:
        fail(f"{label} is not canonical nonzero SHA-256")
    return value


def package_inventory(shards: Path) -> list[str]:
    packages: list[str] = []
    for manifest in sorted(shards.glob("*/Cargo.toml")):
        match = re.search(
            r'^name\s*=\s*"(wm2000-block-[^"]+)"\s*$',
            manifest.read_text(encoding="utf-8"),
            re.MULTILINE,
        )
        if match:
            packages.append(match.group(1))
    packages.sort()
    if len(packages) != 35 or len(set(packages)) != 35:
        fail("shard inventory is not exactly 35 unique packages")
    return packages


def require_plain_directory(path: Path, label: str, private_mode: bool = False) -> None:
    try:
        mode = path.lstat().st_mode
    except OSError:
        fail(f"{label} is unavailable")
    if stat.S_ISLNK(mode) or not stat.S_ISDIR(mode):
        fail(f"{label} is not a non-symlink directory")
    if private_mode and os.name == "posix" and stat.S_IMODE(mode) != 0o700:
        fail(f"{label} does not have private mode 0700")


def require_plain_file(path: Path, label: str, private_mode: bool = False) -> bytes:
    try:
        mode = path.lstat().st_mode
    except OSError:
        fail(f"{label} is unavailable")
    if stat.S_ISLNK(mode) or not stat.S_ISREG(mode):
        fail(f"{label} is not a regular non-symlink file")
    if private_mode and os.name == "posix" and stat.S_IMODE(mode) != 0o600:
        fail(f"{label} does not have private mode 0600")
    try:
        return path.read_bytes()
    except OSError:
        fail(f"{label} cannot be read")


def parse_identity(data: bytes, package: str) -> tuple[str, str]:
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError:
        fail("prepared identity is not UTF-8")
    lines = text.splitlines(keepends=True)
    expected_prefix = [
        f"schema {ARTIFACT_SCHEMA}\n",
        f"package {package}\n",
    ]
    if len(lines) != 4 or lines[:2] != expected_prefix:
        fail("prepared identity has noncanonical schema or package fields")
    values: list[str] = []
    for line, field in zip(lines[2:], ("runner_sha256", "metadata_sha256")):
        prefix = f"{field} "
        if not line.startswith(prefix) or not line.endswith("\n"):
            fail("prepared identity has noncanonical digest fields")
        values.append(canonical_digest(line[len(prefix) : -1], field))
    return values[0], values[1]


def parse_manifest(data: bytes, packages: list[str]) -> dict[str, object]:
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError:
        fail("prepared root manifest is not UTF-8")
    lines = text.splitlines(keepends=True)
    if len(lines) != 7 + len(packages) or any(not line.endswith("\n") for line in lines):
        fail("prepared root manifest has noncanonical line geometry")
    if lines[0] != f"schema {ROOT_SCHEMA}\n":
        fail("prepared root manifest has the wrong schema")
    claims: dict[str, str] = {}
    claim_fields = (
        "normalized_rom_sha256",
        "generator_source_sha256",
        "discovery_source_sha256",
        "emitter_source_sha256",
        "runtime_source_sha256",
    )
    for line, field in zip(lines[1:6], claim_fields):
        prefix = f"{field} "
        if not line.startswith(prefix):
            fail("prepared root manifest has reordered claim fields")
        claims[field] = canonical_digest(line[len(prefix) : -1], field)
    if lines[6] != f"artifact_count {len(packages)}\n":
        fail("prepared root manifest has the wrong artifact count")
    artifacts: dict[str, tuple[str, str, str]] = {}
    for line, package in zip(lines[7:], packages):
        fields = line[:-1].split(" ")
        if len(fields) != 5 or fields[:2] != ["artifact", package]:
            fail("prepared root manifest has noncanonical artifact order")
        artifacts[package] = tuple(
            canonical_digest(value, "artifact digest") for value in fields[2:]
        )  # type: ignore[assignment]
    return {"claims": claims, "artifacts": artifacts}


def validate_tree(root: Path, packages: list[str]) -> tuple[bytes, dict[str, tuple[str, str]]]:
    require_plain_directory(root, "prepared root", private_mode=True)
    entries = sorted(entry.name for entry in root.iterdir())
    if entries != sorted(["manifest.v2", *packages]):
        fail("prepared root topology is not exact")
    manifest_data = require_plain_file(root / "manifest.v2", "prepared root manifest", private_mode=True)
    manifest = parse_manifest(manifest_data, packages)
    identities: dict[str, tuple[str, str]] = {}
    artifacts = manifest["artifacts"]
    assert isinstance(artifacts, dict)
    for package in packages:
        package_root = root / package
        require_plain_directory(package_root, "prepared package", private_mode=True)
        if sorted(entry.name for entry in package_root.iterdir()) != [
            "identity.v1",
            "metadata.rs",
            "runner.rs",
        ]:
            fail("prepared package topology is not exact")
        identity_data = require_plain_file(package_root / "identity.v1", "prepared identity", private_mode=True)
        runner_data = require_plain_file(package_root / "runner.rs", "prepared runner", private_mode=True)
        metadata_data = require_plain_file(package_root / "metadata.rs", "prepared metadata", private_mode=True)
        runner_digest, metadata_digest = parse_identity(identity_data, package)
        observed = (
            digest_bytes(identity_data),
            digest_bytes(runner_data),
            digest_bytes(metadata_data),
        )
        if observed != artifacts[package]:
            fail("prepared root artifact cross-binding is invalid")
        if (runner_digest, metadata_digest) != observed[1:]:
            fail("prepared package sidecar cross-binding is invalid")
        identities[package] = (runner_digest, metadata_digest)
    return manifest_data, identities


def legacy_outputs(target: Path, packages: list[str]) -> dict[str, tuple[bytes, bytes]]:
    outputs: dict[str, tuple[bytes, bytes]] = {}
    build_root = target / "debug" / "build"
    require_plain_directory(build_root, "legacy Cargo build output")
    for package in packages:
        matches = []
        for candidate in build_root.glob(f"{package}-*/out"):
            if (candidate / "runner.rs").is_file() and (candidate / "metadata.rs").is_file():
                matches.append(candidate)
        if len(matches) != 1:
            fail("legacy build does not have exactly one output pair per shard")
        outputs[package] = (
            require_plain_file(matches[0] / "runner.rs", "legacy runner"),
            require_plain_file(matches[0] / "metadata.rs", "legacy metadata"),
        )
    return outputs


def parity_verify(args: argparse.Namespace) -> dict[str, object]:
    shards = Path(args.shards)
    packages = package_inventory(shards)
    legacy = legacy_outputs(Path(args.legacy_target), packages)
    reference_files: dict[tuple[str, str], str] | None = None
    manifest_digest: str | None = None
    aggregate = hashlib.sha256(b"fn64.wm-prepared-parity.v1\0")
    for run in range(args.runs):
        root = Path(args.prepared_parent) / f"publication-{run:02d}"
        manifest_data, identities = validate_tree(root, packages)
        observed_files: dict[tuple[str, str], str] = {}
        for package in packages:
            for name, legacy_data, expected_digest in (
                ("runner.rs", legacy[package][0], identities[package][0]),
                ("metadata.rs", legacy[package][1], identities[package][1]),
            ):
                prepared_data = require_plain_file(root / package / name, "prepared source", private_mode=True)
                if prepared_data != legacy_data or digest_bytes(prepared_data) != expected_digest:
                    fail("prepared and legacy generated output differ")
            for name in ("identity.v1", "runner.rs", "metadata.rs"):
                digest = digest_file(root / package / name)
                observed_files[(package, name)] = digest
                aggregate.update(package.encode())
                aggregate.update(b"\0")
                aggregate.update(name.encode())
                aggregate.update(b"\0")
                aggregate.update(bytes.fromhex(digest))
        current_manifest_digest = digest_bytes(manifest_data)
        aggregate.update(bytes.fromhex(current_manifest_digest))
        if reference_files is None:
            reference_files = observed_files
            manifest_digest = current_manifest_digest
        elif observed_files != reference_files or current_manifest_digest != manifest_digest:
            fail("fresh prepared publications are not byte-deterministic")
    assert manifest_digest is not None
    return {
        "publication_count": args.runs,
        "package_count": len(packages),
        "prepared_package_file_count": args.runs * len(packages) * 3,
        "root_manifest_count": args.runs,
        "legacy_generated_file_count": len(packages) * 2,
        "legacy_parity_comparison_count": args.runs * len(packages) * 2,
        "prepared_manifest_sha256": manifest_digest,
        "parity_evidence_sha256": aggregate.hexdigest(),
    }


def artifact_counts(path: Path, packages: list[str]) -> dict[str, int]:
    library_total = 0
    library_recompiled = 0
    build_script_total = 0
    build_script_reran = 0
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError:
        fail("Cargo JSON message log cannot be read")
    for line in lines:
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            fail("Cargo JSON message log contains a non-JSON line")
        if message.get("reason") != "compiler-artifact":
            continue
        package_id = str(message.get("package_id", ""))
        package = next((candidate for candidate in packages if candidate in package_id), None)
        if package is None:
            continue
        target = message.get("target", {})
        kinds = target.get("kind", []) if isinstance(target, dict) else []
        fresh = message.get("fresh") is True
        if "lib" in kinds:
            library_total += 1
            library_recompiled += int(not fresh)
        if "custom-build" in kinds:
            build_script_total += 1
            build_script_reran += int(not fresh)
    return {
        "shard_library_artifact_count": library_total,
        "shard_rustc_count": library_recompiled,
        "shard_build_script_artifact_count": build_script_total,
        "shard_build_script_run_count": build_script_reran,
    }


def guard_measurement(path: Path) -> dict[str, int]:
    last = None
    peak = 0
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError:
        fail("memory-guard JSONL cannot be read")
    for line in lines:
        try:
            sample = json.loads(line)
        except json.JSONDecodeError:
            fail("memory-guard JSONL contains a non-JSON line")
        if not isinstance(sample, dict) or sample.get("schema") != "fn64.memory-guard.sample.v1":
            fail("memory-guard JSONL has an unknown schema")
        try:
            peak = max(peak, int(sample["peak_tree_rss_mib"]))
        except (KeyError, TypeError, ValueError):
            fail("memory-guard JSONL has malformed measurements")
        last = sample
    if last is None or last.get("reason") != "complete":
        fail("memory-guard JSONL lacks a completed terminal sample")
    try:
        return {
            "elapsed_seconds": int(last["elapsed_seconds"]),
            "peak_tree_rss_mib": peak,
            "final_free_percent": int(last["free_percent"]),
        }
    except (KeyError, TypeError, ValueError):
        fail("memory-guard JSONL has malformed terminal measurements")


def atomic_write(path: Path, data: bytes) -> None:
    temporary = path.with_name(path.name + ".audit-new")
    if temporary.exists():
        fail("private audit temporary file already exists")
    with temporary.open("xb") as sink:
        os.chmod(temporary, 0o600)
        sink.write(data)
        sink.flush()
        os.fsync(sink.fileno())
    os.replace(temporary, path)


def stage_private_file(source: Path, destination: Path) -> None:
    if destination.exists() or destination.is_symlink():
        fail("private staged input already exists")
    flags = os.O_RDONLY
    if hasattr(os, "O_CLOEXEC"):
        flags |= os.O_CLOEXEC
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    try:
        descriptor = os.open(source, flags)
    except OSError:
        fail("private input cannot be opened without following a symlink")
    temporary = destination.with_name(destination.name + ".audit-new")
    try:
        before = os.fstat(descriptor)
        if not stat.S_ISREG(before.st_mode):
            fail("private input is not a regular file")
        with temporary.open("xb") as sink:
            os.chmod(temporary, 0o600)
            copied = 0
            while True:
                chunk = os.read(descriptor, 1024 * 1024)
                if not chunk:
                    break
                sink.write(chunk)
                copied += len(chunk)
            sink.flush()
            os.fsync(sink.fileno())
        after = os.fstat(descriptor)
        stable_fields = ("st_dev", "st_ino", "st_size", "st_mtime_ns", "st_ctime_ns")
        if copied != before.st_size or any(getattr(before, field) != getattr(after, field) for field in stable_fields):
            fail("private input changed while it was staged")
        os.replace(temporary, destination)
    finally:
        os.close(descriptor)
        if temporary.exists():
            temporary.unlink()


def mutate_root_claim(root: Path, packages: list[str]) -> None:
    manifest_data, _ = validate_tree(root, packages)
    lines = manifest_data.decode().splitlines(keepends=True)
    prefix = "generator_source_sha256 "
    if not lines[2].startswith(prefix):
        fail("prepared root claim mutation found an unexpected manifest")
    old = lines[2][len(prefix) : -1]
    replacement = digest_bytes(b"fn64.wm-root-claim-benchmark.v1\0" + bytes.fromhex(old))
    lines[2] = prefix + replacement + "\n"
    atomic_write(root / "manifest.v2", "".join(lines).encode())
    validate_tree(root, packages)


def mutate_one_artifact(root: Path, packages: list[str]) -> None:
    manifest_data, _ = validate_tree(root, packages)
    package = packages[0]
    package_root = root / package
    # One trailing blank line is a semantic no-op in generated Rust. Mutating
    # metadata rather than the runner preserves metadata's embedded binding to
    # the exact runner source while still exercising one package watch set.
    metadata = require_plain_file(package_root / "metadata.rs", "prepared metadata", private_mode=True) + b"\n"
    metadata_digest = digest_bytes(metadata)
    identity_data = require_plain_file(package_root / "identity.v1", "prepared identity", private_mode=True)
    identity_lines = identity_data.decode().splitlines(keepends=True)
    identity_lines[3] = f"metadata_sha256 {metadata_digest}\n"
    new_identity = "".join(identity_lines).encode()
    identity_digest = digest_bytes(new_identity)
    manifest_lines = manifest_data.decode().splitlines(keepends=True)
    artifact_index = 7
    fields = manifest_lines[artifact_index][:-1].split(" ")
    if fields[:2] != ["artifact", package]:
        fail("prepared artifact mutation found an unexpected manifest")
    fields[2] = identity_digest
    fields[4] = metadata_digest
    manifest_lines[artifact_index] = " ".join(fields) + "\n"
    atomic_write(package_root / "metadata.rs", metadata)
    atomic_write(package_root / "identity.v1", new_identity)
    atomic_write(root / "manifest.v2", "".join(manifest_lines).encode())
    validate_tree(root, packages)


def copy_tree(source: Path, destination: Path, packages: list[str]) -> None:
    validate_tree(source, packages)
    if destination.exists() or destination.is_symlink():
        fail("private benchmark working root already exists")
    shutil.copytree(source, destination, symlinks=False)
    validate_tree(destination, packages)


def activation_status(shards: Path) -> bool:
    packages = package_inventory(shards)
    for package in packages:
        manifests = list(shards.glob(f"*/Cargo.toml"))
        manifest = next(
            (
                path
                for path in manifests
                if re.search(rf'^name\s*=\s*"{re.escape(package)}"\s*$', path.read_text(), re.M)
            ),
            None,
        )
        if manifest is None:
            fail("cannot map shard package to manifest")
        text = manifest.read_text(encoding="utf-8")
        if not re.search(r'^build\s*=\s*"\.\./prepared_build\.rs"\s*$', text, re.M):
            return False
        if re.search(r"^\[build-dependencies\]\s*$", text, re.M):
            return False
    return True


def metadata_graph(path: Path, packages: list[str]) -> dict[str, int]:
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        fail("Cargo metadata JSON cannot be parsed")
    package_by_id = {item["id"]: item["name"] for item in document.get("packages", [])}
    nodes = {item["id"]: item for item in document.get("resolve", {}).get("nodes", [])}
    roots = [identifier for identifier, name in package_by_id.items() if name in packages]
    if len(roots) != len(packages):
        fail("Cargo metadata does not contain all 35 shard roots")
    direct_forbidden = {"fn64-discover", "fn64-recomp-rs-codegen", "sha2"}
    for identifier in roots:
        node = nodes.get(identifier)
        if node is None:
            fail("Cargo metadata resolve graph omits a shard root")
        deps = node.get("deps", [])
        if any(str(dependency.get("name", "")).replace("_", "-") in direct_forbidden for dependency in deps):
            fail("activated shard has a direct discovery, codegen, or sha2 dependency")
    reachable: set[str] = set()
    pending = list(roots)
    while pending:
        identifier = pending.pop()
        if identifier in reachable:
            continue
        reachable.add(identifier)
        node = nodes.get(identifier)
        if node is None:
            fail("Cargo metadata resolve graph omits a reachable node")
        dependencies = node.get("dependencies", [])
        pending.extend(str(dependency) for dependency in dependencies)
    # The execution runtime legitimately uses sha2. Stage B removes sha2 from
    # each shard's own build edge; it does not rewrite runtime dependencies.
    forbidden = {"fn64-discover", "fn64-recomp-rs-codegen"}
    observed = forbidden.intersection(package_by_id.get(identifier, "") for identifier in reachable)
    if observed:
        fail("activated shard dependency graph retains discovery or codegen")
    return {
        "shard_root_count": len(roots),
        "reachable_package_count": len(reachable),
        "forbidden_direct_dependency_count": 0,
        "forbidden_codegen_reachable_count": 0,
    }


def validate_locations(args: argparse.Namespace) -> None:
    repo = Path(args.repo).resolve(strict=True)
    candidates = [Path(value) for value in args.outside]
    for candidate in candidates:
        if not candidate.is_absolute():
            fail("private input/output paths must be explicit absolute paths")
        probe = candidate.resolve(strict=False)
        if probe == repo or repo in probe.parents:
            fail("private input/output path is inside the repository")
    for value in args.must_exist:
        if not Path(value).exists():
            fail("required private input path is unavailable")
    for value in args.must_be_absent:
        path = Path(value)
        if path.exists() or path.is_symlink():
            fail("fresh private output path already exists")


def phase_summary(json_path: Path, guard_path: Path, packages: list[str]) -> dict[str, int]:
    return {**artifact_counts(json_path, packages), **guard_measurement(guard_path)}


def validate_benchmark_phase_counts(phases: dict[str, dict[str, int]], package_count: int) -> None:
    for name, phase in phases.items():
        if phase["shard_library_artifact_count"] != package_count:
            fail(f"{name} Cargo JSON does not account for all shard library artifacts")
        if phase["shard_build_script_artifact_count"] != package_count:
            fail(f"{name} Cargo JSON does not account for all shard build-script artifacts")
    expected_runs = {
        "cold": (package_count, package_count),
        "noop": (0, 0),
        "root_claim": (0, 0),
        "one_artifact": (1, 1),
    }
    for name, (expected_rustc, expected_build_scripts) in expected_runs.items():
        if phases[name]["shard_rustc_count"] != expected_rustc:
            fail(f"{name} phase has an unexpected shard rustc count")
        if phases[name]["shard_build_script_run_count"] != expected_build_scripts:
            fail(f"{name} phase has an unexpected shard build-script run count")


def compose_parity(args: argparse.Namespace) -> dict[str, object]:
    packages = package_inventory(Path(args.shards))
    evidence = parity_verify(args)
    producer_compile = phase_summary(Path(args.producer_json), Path(args.producer_guard), packages)
    legacy = phase_summary(Path(args.legacy_json), Path(args.legacy_guard), packages)
    if len(args.publication_guard) != args.runs:
        fail("publication guard cardinality does not match the parity run count")
    producer_runs = [guard_measurement(Path(path)) for path in args.publication_guard]
    for field in ("shard_library_artifact_count", "shard_build_script_artifact_count", "shard_rustc_count", "shard_build_script_run_count"):
        if legacy[field] != len(packages):
            fail("fresh legacy Cargo JSON does not account for all shard compilations")
    return {
        "schema": "fn64.wm-prepared-parity-audit.v1",
        "status": "completed",
        **evidence,
        "producer_compile_elapsed_seconds": producer_compile["elapsed_seconds"],
        "producer_compile_peak_tree_rss_mib": producer_compile["peak_tree_rss_mib"],
        "producer_publication_elapsed_seconds": sum(item["elapsed_seconds"] for item in producer_runs),
        "producer_publication_peak_tree_rss_mib": max(item["peak_tree_rss_mib"] for item in producer_runs),
        "legacy_build_elapsed_seconds": legacy["elapsed_seconds"],
        "legacy_build_peak_tree_rss_mib": legacy["peak_tree_rss_mib"],
        "legacy_shard_rustc_count": legacy["shard_rustc_count"],
    }


def compose_benchmark(args: argparse.Namespace) -> dict[str, object]:
    packages = package_inventory(Path(args.shards))
    phases = {
        name: phase_summary(Path(json_path), Path(guard_path), packages)
        for name, json_path, guard_path in (
            ("cold", args.cold_json, args.cold_guard),
            ("noop", args.noop_json, args.noop_guard),
            ("root_claim", args.root_json, args.root_guard),
            ("one_artifact", args.artifact_json, args.artifact_guard),
        )
    }
    validate_benchmark_phase_counts(phases, len(packages))
    if phases["noop"]["shard_rustc_count"] != 0:
        fail("no-op prepared build recompiled a shard")
    if phases["root_claim"]["shard_rustc_count"] != 0:
        fail("root-claim-only update recompiled a shard")
    if phases["one_artifact"]["shard_rustc_count"] != 1:
        fail("one-artifact update did not recompile exactly one shard")
    graph = metadata_graph(Path(args.metadata), packages)
    manifest_digest = digest_file(Path(args.prepared_work) / "manifest.v2")
    aggregate = hashlib.sha256(b"fn64.wm-prepared-invalidation.v1\0")
    aggregate.update(bytes.fromhex(manifest_digest))
    for name in ("cold", "noop", "root_claim", "one_artifact"):
        aggregate.update(name.encode())
        aggregate.update(json.dumps(phases[name], sort_keys=True, separators=(",", ":")).encode())
    return {
        "schema": "fn64.wm-prepared-invalidation-benchmark.v1",
        "status": "completed",
        "package_count": len(packages),
        "cold": phases["cold"],
        "noop": phases["noop"],
        "root_claim_only": phases["root_claim"],
        "one_artifact_update": phases["one_artifact"],
        "metadata_graph": graph,
        "final_prepared_manifest_sha256": manifest_digest,
        "benchmark_evidence_sha256": aggregate.hexdigest(),
    }


def fixture_tree(root: Path, packages: list[str]) -> None:
    root.mkdir(mode=0o700)
    artifacts = []
    for package in packages:
        package_root = root / package
        package_root.mkdir(mode=0o700)
        runner = f"// runner {package}\n".encode()
        metadata = f"// metadata {package}\n".encode()
        identity = (
            f"schema {ARTIFACT_SCHEMA}\npackage {package}\n"
            f"runner_sha256 {digest_bytes(runner)}\n"
            f"metadata_sha256 {digest_bytes(metadata)}\n"
        ).encode()
        for name, data in (("identity.v1", identity), ("runner.rs", runner), ("metadata.rs", metadata)):
            (package_root / name).write_bytes(data)
            os.chmod(package_root / name, 0o600)
        artifacts.append(
            f"artifact {package} {digest_bytes(identity)} {digest_bytes(runner)} {digest_bytes(metadata)}\n"
        )
    manifest = (
        f"schema {ROOT_SCHEMA}\n"
        + "normalized_rom_sha256 " + "11" * 32 + "\n"
        + "generator_source_sha256 " + "22" * 32 + "\n"
        + "discovery_source_sha256 " + "33" * 32 + "\n"
        + "emitter_source_sha256 " + "44" * 32 + "\n"
        + "runtime_source_sha256 " + "55" * 32 + "\n"
        + f"artifact_count {len(packages)}\n"
        + "".join(artifacts)
    ).encode()
    (root / "manifest.v2").write_bytes(manifest)
    os.chmod(root / "manifest.v2", 0o600)


def selftest(shards: Path) -> None:
    packages = package_inventory(shards)
    with tempfile.TemporaryDirectory(prefix="fn64-wm-audit-selftest.") as temporary:
        base = Path(temporary)
        tree = base / "tree"
        fixture_tree(tree, packages)
        validate_tree(tree, packages)
        prepared_parent = base / "publications"
        prepared_parent.mkdir(mode=0o700)
        for run in range(2):
            copy_tree(tree, prepared_parent / f"publication-{run:02d}", packages)
        legacy_target = base / "legacy"
        for package in packages:
            output = legacy_target / "debug" / "build" / f"{package}-fixture" / "out"
            output.mkdir(parents=True, mode=0o700)
            for name in ("runner.rs", "metadata.rs"):
                data = (tree / package / name).read_bytes()
                (output / name).write_bytes(data)
                os.chmod(output / name, 0o600)
        parity_result = parity_verify(
            argparse.Namespace(
                shards=str(shards),
                legacy_target=str(legacy_target),
                prepared_parent=str(prepared_parent),
                runs=2,
            )
        )
        if parity_result["legacy_parity_comparison_count"] != 2 * len(packages) * 2:
            fail("schema-aware legacy parity fixture self-test failed")
        private_input = base / "private-input"
        private_input.write_bytes(b"private fixture bytes")
        staged_input = base / "staged-input"
        stage_private_file(private_input, staged_input)
        if staged_input.read_bytes() != private_input.read_bytes():
            fail("descriptor-stable private input staging self-test failed")
        if hasattr(os, "symlink"):
            input_link = base / "private-input-link"
            input_link.symlink_to(private_input)
            try:
                stage_private_file(input_link, base / "rejected-staged-input")
            except AuditError:
                pass
            else:
                fail("private input symlink negative self-test failed")
        copied = base / "copy"
        copy_tree(tree, copied, packages)
        mutate_root_claim(copied, packages)
        mutate_one_artifact(copied, packages)
        cargo_json = base / "cargo.json"
        cargo_json.write_text(
            "\n".join(
                json.dumps(
                    {
                        "reason": "compiler-artifact",
                        "package_id": f"path+file:///private#{packages[index]}@0.0.0",
                        "target": {"kind": ["lib"]},
                        "fresh": fresh,
                    }
                )
                for index, fresh in ((0, False), (1, True))
            )
            + "\n"
        )
        counts = artifact_counts(cargo_json, packages)
        if counts["shard_rustc_count"] != 1 or counts["shard_library_artifact_count"] != 2:
            fail("Cargo compiler-artifact parser self-test failed")
        empty_cargo = base / "empty-cargo.json"
        empty_cargo.write_text("", encoding="utf-8")
        if artifact_counts(empty_cargo, packages)["shard_library_artifact_count"] != 0:
            fail("vacuous Cargo compiler-artifact fixture was not empty")
        complete_phase = {
            "shard_library_artifact_count": len(packages),
            "shard_build_script_artifact_count": len(packages),
            "shard_rustc_count": 0,
            "shard_build_script_run_count": 0,
        }
        phase_fixture = {
            "cold": {
                **complete_phase,
                "shard_rustc_count": len(packages),
                "shard_build_script_run_count": len(packages),
            },
            "noop": dict(complete_phase),
            "root_claim": dict(complete_phase),
            "one_artifact": {
                **complete_phase,
                "shard_rustc_count": 1,
                "shard_build_script_run_count": 1,
            },
        }
        validate_benchmark_phase_counts(phase_fixture, len(packages))
        phase_fixture["noop"]["shard_library_artifact_count"] = 0
        try:
            validate_benchmark_phase_counts(phase_fixture, len(packages))
        except AuditError:
            pass
        else:
            fail("vacuous benchmark composition negative self-test failed")
        malformed_cargo = base / "malformed-cargo.json"
        malformed_cargo.write_text("not JSON\n", encoding="utf-8")
        try:
            artifact_counts(malformed_cargo, packages)
        except AuditError:
            pass
        else:
            fail("malformed Cargo compiler-artifact negative self-test failed")
        guard = base / "guard.jsonl"
        guard.write_text(
            json.dumps(
                {
                    "schema": "fn64.memory-guard.sample.v1",
                    "elapsed_seconds": 7,
                    "tree_rss_mib": 12,
                    "peak_tree_rss_mib": 34,
                    "largest_process_rss_mib": 10,
                    "free_percent": 56,
                    "reason": "complete",
                }
            )
            + "\n",
            encoding="utf-8",
        )
        if guard_measurement(guard) != {
            "elapsed_seconds": 7,
            "peak_tree_rss_mib": 34,
            "final_free_percent": 56,
        }:
            fail("memory-guard JSONL parser self-test failed")
        metadata_path = base / "metadata.json"
        package_rows = []
        node_rows = []
        for index, package in enumerate(packages):
            identifier = f"path+file:///private#{package}@0.0.0"
            runtime_identifier = "path+file:///private#fn64-recomp-rs@0.0.0"
            package_rows.append({"id": identifier, "name": package})
            node_rows.append(
                {
                    "id": identifier,
                    "dependencies": [runtime_identifier],
                    "deps": [
                        {
                            "name": "fn64_recomp_rs",
                            "pkg": runtime_identifier,
                            "dep_kinds": [{"kind": None, "target": None}],
                        }
                    ],
                }
            )
        package_rows.extend(
            [
                {"id": "path+file:///private#fn64-recomp-rs@0.0.0", "name": "fn64-recomp-rs"},
                {"id": "registry+private#sha2@0.10.0", "name": "sha2"},
            ]
        )
        node_rows.extend(
            [
                {
                    "id": "path+file:///private#fn64-recomp-rs@0.0.0",
                    "dependencies": ["registry+private#sha2@0.10.0"],
                    "deps": [],
                },
                {"id": "registry+private#sha2@0.10.0", "dependencies": [], "deps": []},
            ]
        )
        metadata_document = {"packages": package_rows, "resolve": {"nodes": node_rows}}
        metadata_path.write_text(json.dumps(metadata_document), encoding="utf-8")
        if metadata_graph(metadata_path, packages)["forbidden_direct_dependency_count"] != 0:
            fail("Cargo metadata dependency-kind parser self-test failed")
        node_rows[0]["deps"].append(
            {
                "name": "sha2",
                "pkg": "registry+private#sha2@0.10.0",
                "dep_kinds": [{"kind": "build", "target": None}],
            }
        )
        metadata_path.write_text(json.dumps(metadata_document), encoding="utf-8")
        try:
            metadata_graph(metadata_path, packages)
        except AuditError:
            pass
        else:
            fail("direct shard sha2 dependency negative self-test failed")
        for invalid_line in (
            "not JSON\n",
            json.dumps({"schema": "unknown", "reason": "complete"}) + "\n",
            json.dumps(
                {
                    "schema": "fn64.memory-guard.sample.v1",
                    "elapsed_seconds": 1,
                    "peak_tree_rss_mib": 1,
                    "free_percent": 50,
                    "reason": "sample",
                }
            )
            + "\n",
        ):
            invalid_guard = base / "invalid-guard.jsonl"
            invalid_guard.write_text(invalid_line, encoding="utf-8")
            try:
                guard_measurement(invalid_guard)
            except (AuditError, KeyError):
                pass
            else:
                fail("memory-guard JSONL negative self-test failed")
        if os.name == "posix":
            os.chmod(copied / packages[0] / "runner.rs", 0o644)
            try:
                validate_tree(copied, packages)
            except AuditError:
                pass
            else:
                fail("non-private prepared file mode negative self-test failed")
            os.chmod(copied / packages[0] / "runner.rs", 0o600)
            os.chmod(copied / packages[0], 0o755)
            try:
                validate_tree(copied, packages)
            except AuditError:
                pass
            else:
                fail("non-private prepared directory mode negative self-test failed")
        bad = bytearray((tree / "manifest.v2").read_bytes())
        bad[-2] ^= 1
        (tree / "manifest.v2").write_bytes(bad)
        try:
            validate_tree(tree, packages)
        except AuditError:
            pass
        else:
            fail("prepared cross-binding negative self-test failed")


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(add_help=False)
    sub = result.add_subparsers(dest="command", required=True)
    inventory = sub.add_parser("activation", add_help=False)
    inventory.add_argument("--shards", required=True)
    locations = sub.add_parser("validate-locations", add_help=False)
    locations.add_argument("--repo", required=True)
    locations.add_argument("--outside", action="append", default=[])
    locations.add_argument("--must-exist", action="append", default=[])
    locations.add_argument("--must-be-absent", action="append", default=[])
    parity = sub.add_parser("parity", add_help=False)
    parity.add_argument("--shards", required=True)
    parity.add_argument("--legacy-target", required=True)
    parity.add_argument("--prepared-parent", required=True)
    parity.add_argument("--runs", type=int, required=True)
    copy = sub.add_parser("copy-tree", add_help=False)
    copy.add_argument("--shards", required=True)
    copy.add_argument("--source", required=True)
    copy.add_argument("--destination", required=True)
    stage = sub.add_parser("stage-private-file", add_help=False)
    stage.add_argument("--source", required=True)
    stage.add_argument("--destination", required=True)
    root_claim = sub.add_parser("mutate-root-claim", add_help=False)
    root_claim.add_argument("--shards", required=True)
    root_claim.add_argument("--root", required=True)
    artifact = sub.add_parser("mutate-one-artifact", add_help=False)
    artifact.add_argument("--shards", required=True)
    artifact.add_argument("--root", required=True)
    compose_p = sub.add_parser("compose-parity", add_help=False)
    for option in ("shards", "legacy-target", "prepared-parent", "producer-json", "producer-guard", "legacy-json", "legacy-guard"):
        compose_p.add_argument("--" + option, required=True)
    compose_p.add_argument("--runs", type=int, required=True)
    compose_p.add_argument("--publication-guard", action="append", required=True)
    compose_b = sub.add_parser("compose-benchmark", add_help=False)
    for option in (
        "shards", "cold-json", "cold-guard", "noop-json", "noop-guard",
        "root-json", "root-guard", "artifact-json", "artifact-guard",
        "metadata", "prepared-work",
    ):
        compose_b.add_argument("--" + option, required=True)
    test = sub.add_parser("selftest", add_help=False)
    test.add_argument("--shards", required=True)
    return result


def main() -> int:
    args = parser().parse_args()
    if args.command == "activation":
        print("active" if activation_status(Path(args.shards)) else "inactive")
    elif args.command == "validate-locations":
        validate_locations(args)
    elif args.command == "parity":
        print(json.dumps(parity_verify(args), sort_keys=True, separators=(",", ":")))
    elif args.command == "copy-tree":
        packages = package_inventory(Path(args.shards))
        copy_tree(Path(args.source), Path(args.destination), packages)
    elif args.command == "stage-private-file":
        stage_private_file(Path(args.source), Path(args.destination))
    elif args.command == "mutate-root-claim":
        packages = package_inventory(Path(args.shards))
        mutate_root_claim(Path(args.root), packages)
    elif args.command == "mutate-one-artifact":
        packages = package_inventory(Path(args.shards))
        mutate_one_artifact(Path(args.root), packages)
    elif args.command == "compose-parity":
        print(json.dumps(compose_parity(args), sort_keys=True, separators=(",", ":")))
    elif args.command == "compose-benchmark":
        print(json.dumps(compose_benchmark(args), sort_keys=True, separators=(",", ":")))
    elif args.command == "selftest":
        selftest(Path(args.shards))
        print("wm prepared audit parser self-test: PASS")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except AuditError as error:
        print(f"wm prepared audit: {error}", file=sys.stderr)
        raise SystemExit(3)
