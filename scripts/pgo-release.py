#!/usr/bin/env python3
"""Build digest-bound ordinary or profile-guided release artifacts.

The workflow is game-neutral.  A private, explicit manifest supplies the Cargo
build and training commands; this script owns isolation, PGO flags, profile
merging, and compatibility receipts.  It never discovers a game checkout or a
training route from the local machine.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import signal
import stat
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any


MANIFEST_SCHEMA = "fn64.pgo-training-manifest.v1"
PROFILE_RECEIPT_SCHEMA = "fn64.pgo-profile-receipt.v1"
BUILD_RECEIPT_SCHEMA = "fn64.pgo-build-receipt.v1"
MAX_MANIFEST_BYTES = 1024 * 1024
MAX_IDENTITY_FILE_BYTES = 1024 * 1024 * 1024
MAX_PROFILE_BYTES = 1024 * 1024 * 1024
MAX_ARTIFACT_BYTES = 2 * 1024 * 1024 * 1024
IDENTIFIER = re.compile(r"[a-z0-9][a-z0-9_-]{0,63}\Z")
ENVIRONMENT_NAME = re.compile(r"[A-Za-z_][A-Za-z0-9_]*\Z")
PLACEHOLDER = re.compile(r"\{([a-z_]+)\}")
CONTROLLED_ENVIRONMENT = {
    "CARGO_ENCODED_RUSTFLAGS",
    "CARGO_INCREMENTAL",
    "CARGO_TARGET_DIR",
    "LLVM_PROFILE_FILE",
    "RUSTFLAGS",
}
REPO_ROOT = Path(__file__).resolve().parent.parent


class PgoError(RuntimeError):
    pass


@dataclass(frozen=True)
class Toolchain:
    cargo: tuple[str, ...]
    rustc: tuple[str, ...]


@dataclass(frozen=True)
class BuildSpec:
    arguments: tuple[str, ...]
    cwd: Path
    artifact_template: str
    rustflags: tuple[str, ...]
    environment: dict[str, str]
    inherit_environment: tuple[str, ...]


@dataclass(frozen=True)
class TrainingCommand:
    identifier: str
    command: tuple[str, ...]
    cwd: Path
    environment: dict[str, str]
    inherit_environment: tuple[str, ...]


@dataclass(frozen=True)
class IdentityFile:
    identifier: str
    path: Path


@dataclass(frozen=True)
class Manifest:
    path: Path
    digest: str
    profile_id: str
    target: str
    toolchain: Toolchain
    build: BuildSpec
    training: tuple[TrainingCommand, ...]
    identity_files: tuple[IdentityFile, ...]


def exact_keys(value: dict[str, Any], required: set[str], label: str) -> None:
    missing = required - set(value)
    unknown = set(value) - required
    if missing or unknown:
        raise PgoError(
            f"{label} fields differ: missing={sorted(missing)} unknown={sorted(unknown)}"
        )


def reject_duplicate_pairs(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise PgoError(f"JSON contains duplicate field {key!r}")
        result[key] = value
    return result


def decode_json(data: bytes, label: str) -> Any:
    try:
        return json.loads(
            data.decode("utf-8"),
            object_pairs_hook=reject_duplicate_pairs,
            parse_constant=lambda value: (_ for _ in ()).throw(
                PgoError(f"{label} contains non-finite number {value}")
            ),
        )
    except UnicodeDecodeError as error:
        raise PgoError(f"{label} is not UTF-8") from error
    except json.JSONDecodeError as error:
        raise PgoError(f"{label} is not valid JSON") from error


def object_value(value: Any, label: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise PgoError(f"{label} must be a JSON object")
    return value


def string_value(value: Any, label: str) -> str:
    if not isinstance(value, str) or not value:
        raise PgoError(f"{label} must be a non-empty string")
    if "\x00" in value:
        raise PgoError(f"{label} contains NUL")
    return value


def identifier_value(value: Any, label: str) -> str:
    text = string_value(value, label)
    if IDENTIFIER.fullmatch(text) is None:
        raise PgoError(f"{label} must match {IDENTIFIER.pattern}")
    return text


def string_list(value: Any, label: str) -> tuple[str, ...]:
    if not isinstance(value, list) or not value:
        raise PgoError(f"{label} must be a non-empty JSON array")
    return tuple(string_value(item, f"{label}[{index}]") for index, item in enumerate(value))


def environment_value(value: Any, label: str) -> dict[str, str]:
    mapping = object_value(value, label)
    result: dict[str, str] = {}
    for key, item in mapping.items():
        key = string_value(key, f"{label} key")
        if "=" in key:
            raise PgoError(f"{label} key {key!r} contains '='")
        if key in CONTROLLED_ENVIRONMENT:
            raise PgoError(f"{label} may not override workflow-owned {key}")
        result[key] = string_value(item, f"{label}.{key}")
    return result


def inherited_environment_value(value: Any, label: str) -> tuple[str, ...]:
    if not isinstance(value, list):
        raise PgoError(f"{label} must be a JSON array")
    result = []
    for index, item in enumerate(value):
        name = string_value(item, f"{label}[{index}]")
        if ENVIRONMENT_NAME.fullmatch(name) is None:
            raise PgoError(f"{label}[{index}] is not an environment variable name")
        if name in CONTROLLED_ENVIRONMENT:
            raise PgoError(f"{label} may not inherit workflow-owned {name}")
        result.append(name)
    if len(result) != len(set(result)):
        raise PgoError(f"{label} contains duplicate names")
    return tuple(result)


def canonical_json(value: Any) -> bytes:
    return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def stable_file(path: Path, label: str, maximum: int) -> tuple[str, int]:
    try:
        before = path.lstat()
    except OSError as error:
        raise PgoError(f"cannot inspect {label} {path}: {error}") from error
    if stat.S_ISLNK(before.st_mode) or not stat.S_ISREG(before.st_mode):
        raise PgoError(f"{label} must be a non-symlink regular file: {path}")
    if before.st_size > maximum:
        raise PgoError(f"{label} exceeds {maximum} bytes: {path}")
    digest = hashlib.sha256()
    try:
        with path.open("rb") as handle:
            while chunk := handle.read(1024 * 1024):
                digest.update(chunk)
        after = path.lstat()
    except OSError as error:
        raise PgoError(f"cannot read {label} {path}: {error}") from error
    identity_before = (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns)
    identity_after = (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns)
    if identity_before != identity_after:
        raise PgoError(f"{label} changed while it was being hashed: {path}")
    return digest.hexdigest(), before.st_size


def resolve_path(text: str, manifest_dir: Path, label: str) -> Path:
    raw = Path(string_value(text, label)).expanduser()
    candidate = raw if raw.is_absolute() else manifest_dir / raw
    try:
        return candidate.resolve(strict=True)
    except OSError as error:
        raise PgoError(f"cannot resolve {label} {candidate}: {error}") from error


def load_manifest(path: Path) -> Manifest:
    path = path.expanduser().resolve(strict=True)
    digest, size = stable_file(path, "PGO manifest", MAX_MANIFEST_BYTES)
    data = path.read_bytes()
    if len(data) != size:
        raise PgoError("PGO manifest changed after hashing")
    value = object_value(decode_json(data, "PGO manifest"), "PGO manifest")
    exact_keys(
        value,
        {
            "schema",
            "schema_version",
            "profile_id",
            "target",
            "toolchain",
            "build",
            "training",
            "identity_files",
        },
        "PGO manifest",
    )
    if value["schema"] != MANIFEST_SCHEMA or value["schema_version"] != 1:
        raise PgoError("unsupported PGO manifest schema")
    profile_id = identifier_value(value["profile_id"], "profile_id")
    target = string_value(value["target"], "target")

    tools = object_value(value["toolchain"], "toolchain")
    exact_keys(tools, {"cargo", "rustc"}, "toolchain")
    toolchain = Toolchain(
        cargo=string_list(tools["cargo"], "toolchain.cargo"),
        rustc=string_list(tools["rustc"], "toolchain.rustc"),
    )

    build_value = object_value(value["build"], "build")
    exact_keys(
        build_value,
        {
            "arguments",
            "cwd",
            "artifact",
            "rustflags",
            "environment",
            "inherit_environment",
        },
        "build",
    )
    arguments = string_list(build_value["arguments"], "build.arguments")
    if "build" not in arguments or "--release" not in arguments:
        raise PgoError("build.arguments must select Cargo 'build' and '--release'")
    if "--locked" not in arguments and "--frozen" not in arguments:
        raise PgoError("build.arguments must contain '--locked' or '--frozen'")
    if arguments.count("--target") != 1:
        raise PgoError("build.arguments must contain '--target' so build scripts are not instrumented")
    target_index = arguments.index("--target")
    if target_index + 1 == len(arguments):
        raise PgoError("build.arguments '--target' is missing its value")
    declared_target = arguments[target_index + 1].replace("{target}", target)
    if declared_target != target:
        raise PgoError("build.arguments '--target' value must equal the manifest target")
    if any(argument == "--target-dir" or argument.startswith("--target-dir=") for argument in arguments):
        raise PgoError("build.arguments may not override workflow-owned target directory")
    build = BuildSpec(
        arguments=arguments,
        cwd=resolve_path(build_value["cwd"], path.parent, "build.cwd"),
        artifact_template=string_value(build_value["artifact"], "build.artifact"),
        rustflags=tuple(
            string_value(item, f"build.rustflags[{index}]")
            for index, item in enumerate(build_value["rustflags"])
        ) if isinstance(build_value["rustflags"], list) else (_ for _ in ()).throw(
            PgoError("build.rustflags must be a JSON array")
        ),
        environment=environment_value(build_value["environment"], "build.environment"),
        inherit_environment=inherited_environment_value(
            build_value["inherit_environment"], "build.inherit_environment"
        ),
    )
    if any("\x1f" in flag for flag in build.rustflags):
        raise PgoError("build.rustflags contains Cargo's encoded flag separator")
    if any("profile-generate" in flag or "profile-use" in flag for flag in build.rustflags):
        raise PgoError("build.rustflags may not supply workflow-owned PGO flags")

    training_value = value["training"]
    if not isinstance(training_value, list) or not training_value:
        raise PgoError("training must be a non-empty JSON array")
    training: list[TrainingCommand] = []
    for index, raw in enumerate(training_value):
        item = object_value(raw, f"training[{index}]")
        exact_keys(
            item,
            {"id", "command", "cwd", "environment", "inherit_environment"},
            f"training[{index}]",
        )
        training.append(
            TrainingCommand(
                identifier=identifier_value(item["id"], f"training[{index}].id"),
                command=string_list(item["command"], f"training[{index}].command"),
                cwd=resolve_path(item["cwd"], path.parent, f"training[{index}].cwd"),
                environment=environment_value(item["environment"], f"training[{index}].environment"),
                inherit_environment=inherited_environment_value(
                    item["inherit_environment"],
                    f"training[{index}].inherit_environment",
                ),
            )
        )
    ids = [item.identifier for item in training]
    if len(ids) != len(set(ids)):
        raise PgoError("training command ids must be unique")

    identities_value = value["identity_files"]
    if not isinstance(identities_value, list) or not identities_value:
        raise PgoError("identity_files must be a non-empty JSON array")
    identity_files: list[IdentityFile] = []
    for index, raw in enumerate(identities_value):
        item = object_value(raw, f"identity_files[{index}]")
        exact_keys(item, {"id", "path"}, f"identity_files[{index}]")
        identity_files.append(
            IdentityFile(
                identifier=identifier_value(item["id"], f"identity_files[{index}].id"),
                path=resolve_path(item["path"], path.parent, f"identity_files[{index}].path"),
            )
        )
    identity_ids = [item.identifier for item in identity_files]
    if len(identity_ids) != len(set(identity_ids)):
        raise PgoError("identity file ids must be unique")

    return Manifest(
        path=path,
        digest=digest,
        profile_id=profile_id,
        target=target,
        toolchain=toolchain,
        build=build,
        training=tuple(training),
        identity_files=tuple(identity_files),
    )


def substitutions(manifest: Manifest, output: Path, target_dir: Path, artifact: Path | None = None) -> dict[str, str]:
    values = {
        "output_dir": str(output),
        "profile_dir": str(output / "raw"),
        "merged_profile": str(output / "merged.profdata"),
        "target": manifest.target,
        "target_dir": str(target_dir),
    }
    if artifact is not None:
        values["artifact"] = str(artifact)
    return values


def expand(text: str, values: dict[str, str], label: str) -> str:
    unknown = set(PLACEHOLDER.findall(text)) - set(values)
    if unknown:
        raise PgoError(f"{label} uses unknown or unavailable placeholders {sorted(unknown)}")
    return PLACEHOLDER.sub(lambda match: values[match.group(1)], text)


def expanded_arguments(arguments: tuple[str, ...], values: dict[str, str], label: str) -> list[str]:
    return [expand(argument, values, f"{label}[{index}]") for index, argument in enumerate(arguments)]


def artifact_path(manifest: Manifest, output: Path, target_dir: Path) -> Path:
    values = substitutions(manifest, output, target_dir)
    expanded = Path(expand(manifest.build.artifact_template, values, "build.artifact")).expanduser()
    candidate = expanded if expanded.is_absolute() else manifest.build.cwd / expanded
    candidate = candidate.resolve(strict=False)
    try:
        candidate.relative_to(target_dir)
    except ValueError as error:
        raise PgoError("build.artifact must resolve inside the workflow-owned target directory") from error
    return candidate


def check_output_location(output: Path) -> Path:
    output = output.expanduser().resolve(strict=False)
    try:
        output.relative_to(REPO_ROOT)
    except ValueError:
        return output
    raise PgoError(f"PGO output must be outside the fn64 repository: {output}")


def prepare_new_output(output: Path) -> None:
    if output.exists():
        if not output.is_dir():
            raise PgoError(f"output exists and is not a directory: {output}")
        if any(output.iterdir()):
            raise PgoError(f"training output must be a new empty directory: {output}")
    else:
        output.mkdir(parents=True)
    (output / "logs").mkdir()
    (output / "raw").mkdir()


def inherited_environment_identity(names: tuple[str, ...]) -> list[dict[str, str]]:
    result = []
    for name in names:
        if name not in os.environ:
            raise PgoError(f"declared inherited environment variable is absent: {name}")
        result.append({"name": name, "value_sha256": sha256_bytes(os.environ[name].encode())})
    return result


def command_environment(
    overrides: dict[str, str], inherited: tuple[str, ...] = ()
) -> dict[str, str]:
    conflicts = sorted(CONTROLLED_ENVIRONMENT & set(os.environ))
    if conflicts:
        raise PgoError(
            "ambient environment contains workflow-owned variables: " + ", ".join(conflicts)
        )
    environment = {}
    for name in inherited:
        if name not in os.environ:
            raise PgoError(f"declared inherited environment variable is absent: {name}")
        environment[name] = os.environ[name]
    environment.update(overrides)
    return environment


def tail(path: Path, limit: int = 16 * 1024) -> str:
    try:
        with path.open("rb") as handle:
            handle.seek(max(0, path.stat().st_size - limit))
            return handle.read().decode("utf-8", errors="replace")
    except OSError:
        return "<log unavailable>"


def run_logged(command: list[str], cwd: Path, environment: dict[str, str], log: Path, timeout: int, label: str) -> None:
    with log.open("wb") as output:
        process = subprocess.Popen(
            command,
            cwd=cwd,
            env=environment,
            stdout=output,
            stderr=subprocess.STDOUT,
            start_new_session=os.name == "posix",
        )
        try:
            return_code = process.wait(timeout=timeout)
        except subprocess.TimeoutExpired as error:
            if os.name == "posix":
                os.killpg(process.pid, signal.SIGKILL)
            else:
                process.kill()
            process.wait()
            raise PgoError(f"{label} timed out after {timeout}s; tail of {log}:\n{tail(log)}") from error
    if return_code != 0:
        raise PgoError(f"{label} exited {return_code}; tail of {log}:\n{tail(log)}")


def tool_output(command: tuple[str, ...] | list[str], suffix: list[str], label: str) -> dict[str, Any]:
    full = [*command, *suffix]
    try:
        result = subprocess.run(full, capture_output=True, timeout=30)
    except (OSError, subprocess.TimeoutExpired) as error:
        raise PgoError(f"cannot run {label} command {full}: {error}") from error
    if result.returncode != 0:
        raise PgoError(f"{label} command failed: {result.stderr.decode(errors='replace')}")
    executable = shutil.which(full[0]) if not Path(full[0]).is_absolute() else full[0]
    if executable is None:
        raise PgoError(f"cannot resolve {label} executable {full[0]}")
    executable_path = Path(executable).resolve(strict=True)
    executable_sha, executable_size = stable_file(executable_path, f"{label} executable", MAX_ARTIFACT_BYTES)
    try:
        output = result.stdout.decode("utf-8", errors="strict").strip()
    except UnicodeDecodeError as error:
        raise PgoError(f"{label} version output is not UTF-8") from error
    return {
        "command": full,
        "executable_sha256": executable_sha,
        "executable_size": executable_size,
        "output_sha256": sha256_bytes(result.stdout),
        "output": output,
    }


def toolchain_identity(manifest: Manifest, llvm_profdata: Path | None = None) -> dict[str, Any]:
    result = {
        "cargo": tool_output(manifest.toolchain.cargo, ["--version", "--verbose"], "Cargo"),
        "rustc": tool_output(manifest.toolchain.rustc, ["-vV"], "rustc"),
    }
    if llvm_profdata is not None:
        result["llvm_profdata"] = tool_output(
            [str(llvm_profdata)], ["--version"], "llvm-profdata"
        )
    return result


def identity_snapshot(manifest: Manifest) -> list[dict[str, Any]]:
    result = []
    for item in manifest.identity_files:
        digest, size = stable_file(item.path, f"identity file {item.identifier}", MAX_IDENTITY_FILE_BYTES)
        result.append({"id": item.identifier, "sha256": digest, "size": size})
    return result


def require_manifest_stable(manifest: Manifest) -> None:
    digest, _ = stable_file(manifest.path, "PGO manifest", MAX_MANIFEST_BYTES)
    if digest != manifest.digest:
        raise PgoError("PGO manifest changed during the workflow")


def require_identity_stable(manifest: Manifest, expected: list[dict[str, Any]]) -> None:
    actual = identity_snapshot(manifest)
    if actual != expected:
        raise PgoError("declared build identity files changed during or since profile training")


def atomic_json(path: Path, value: dict[str, Any]) -> None:
    data = canonical_json(value)
    temporary = path.with_name(path.name + ".new")
    try:
        with temporary.open("xb") as handle:
            handle.write(data)
            handle.flush()
            os.fsync(handle.fileno())
        temporary.replace(path)
    except OSError as error:
        raise PgoError(f"cannot write receipt {path}: {error}") from error


def add_receipt_digest(value: dict[str, Any]) -> dict[str, Any]:
    result = dict(value)
    result["receipt_sha256"] = sha256_bytes(canonical_json(value))
    return result


def llvm_profdata_path(argument: str) -> Path:
    raw = Path(argument).expanduser()
    if raw.is_absolute():
        candidate = raw
    else:
        found = shutil.which(argument)
        if found is None:
            raise PgoError(f"llvm-profdata is not on PATH: {argument}")
        candidate = Path(found)
    candidate = candidate.resolve(strict=True)
    stable_file(candidate, "llvm-profdata executable", MAX_ARTIFACT_BYTES)
    return candidate


def encoded_rustflags(flags: list[str]) -> str:
    if any("\x1f" in flag for flag in flags):
        raise PgoError("a Rust flag contains Cargo's encoded separator")
    return "\x1f".join(flags)


def build_environment(manifest: Manifest, target_dir: Path, pgo_flag: str | None, warning: bool) -> dict[str, str]:
    flags = list(manifest.build.rustflags)
    if pgo_flag is not None:
        flags.append(pgo_flag)
    if warning:
        flags.append("-Cllvm-args=-pgo-warn-missing-function")
    overrides = {
        **manifest.build.environment,
        "CARGO_TARGET_DIR": str(target_dir),
        "CARGO_INCREMENTAL": "0",
        "CARGO_ENCODED_RUSTFLAGS": encoded_rustflags(flags),
    }
    return command_environment(overrides, manifest.build.inherit_environment)


def cargo_build(manifest: Manifest, output: Path, target_dir: Path, pgo_flag: str | None, warning: bool, log_name: str, timeout: int) -> Path:
    values = substitutions(manifest, output, target_dir)
    command = [
        *manifest.toolchain.cargo,
        *expanded_arguments(manifest.build.arguments, values, "build.arguments"),
    ]
    run_logged(
        command,
        manifest.build.cwd,
        build_environment(manifest, target_dir, pgo_flag, warning),
        output / "logs" / log_name,
        timeout,
        log_name,
    )
    artifact = artifact_path(manifest, output, target_dir)
    stable_file(artifact, "release artifact", MAX_ARTIFACT_BYTES)
    return artifact


def train(manifest: Manifest, output: Path, llvm_profdata: Path, timeout: int) -> dict[str, Any]:
    prepare_new_output(output)
    initial_identity = identity_snapshot(manifest)
    build_inherited_environment = inherited_environment_identity(
        manifest.build.inherit_environment
    )
    training_inherited_environment = [
        {
            "id": route.identifier,
            "values": inherited_environment_identity(route.inherit_environment),
        }
        for route in manifest.training
    ]
    tools = toolchain_identity(manifest, llvm_profdata)
    instrumented_target = output / "instrumented-target"
    raw_dir = output / "raw"
    artifact = cargo_build(
        manifest,
        output,
        instrumented_target,
        f"-Cprofile-generate={raw_dir}",
        False,
        "instrumented-build.log",
        timeout,
    )
    artifact_sha, artifact_size = stable_file(artifact, "instrumented artifact", MAX_ARTIFACT_BYTES)

    for route in manifest.training:
        values = substitutions(manifest, output, instrumented_target, artifact)
        command = expanded_arguments(route.command, values, f"training {route.identifier}")
        environment = command_environment(
            {
                **route.environment,
                "LLVM_PROFILE_FILE": str(raw_dir / f"{route.identifier}-%m-%p.profraw"),
            },
            route.inherit_environment,
        )
        before = set(raw_dir.glob(f"{route.identifier}-*.profraw"))
        run_logged(
            command,
            route.cwd,
            environment,
            output / "logs" / f"training-{route.identifier}.log",
            timeout,
            f"training route {route.identifier}",
        )
        after = set(raw_dir.glob(f"{route.identifier}-*.profraw"))
        if not after - before:
            raise PgoError(f"training route {route.identifier} emitted no new .profraw file")

    raw_profiles: list[dict[str, Any]] = []
    raw_paths = sorted(raw_dir.glob("*.profraw"), key=lambda item: item.name)
    if not raw_paths:
        raise PgoError("training emitted no .profraw files")
    for path in raw_paths:
        digest, size = stable_file(path, "raw profile", MAX_PROFILE_BYTES)
        if size == 0:
            raise PgoError(f"raw profile is empty: {path}")
        raw_profiles.append({"name": path.name, "sha256": digest, "size": size})

    merged = output / "merged.profdata"
    run_logged(
        [str(llvm_profdata), "merge", "-o", str(merged), *map(str, raw_paths)],
        output,
        command_environment({}),
        output / "logs" / "llvm-profdata-merge.log",
        timeout,
        "llvm-profdata merge",
    )
    merged_sha, merged_size = stable_file(merged, "merged profile", MAX_PROFILE_BYTES)
    if merged_size == 0:
        raise PgoError("merged profile is empty")
    require_manifest_stable(manifest)
    require_identity_stable(manifest, initial_identity)

    body = {
        "schema": PROFILE_RECEIPT_SCHEMA,
        "schema_version": 1,
        "profile_id": manifest.profile_id,
        "target": manifest.target,
        "manifest_sha256": manifest.digest,
        "toolchain": tools,
        "identity_files": initial_identity,
        "build_inherited_environment": build_inherited_environment,
        "training_inherited_environment": training_inherited_environment,
        "instrumented_artifact": {"sha256": artifact_sha, "size": artifact_size},
        "raw_profiles": raw_profiles,
        "merged_profile": {"sha256": merged_sha, "size": merged_size},
        "training_ids": [route.identifier for route in manifest.training],
        "profile_generate_flag": f"-Cprofile-generate={raw_dir}",
    }
    receipt = add_receipt_digest(body)
    atomic_json(output / "profile-receipt.json", receipt)
    return receipt


def load_profile_receipt(path: Path) -> dict[str, Any]:
    digest, size = stable_file(path, "profile receipt", MAX_MANIFEST_BYTES)
    value = object_value(decode_json(path.read_bytes(), "profile receipt"), "profile receipt")
    exact_keys(
        value,
        {
            "schema",
            "schema_version",
            "profile_id",
            "target",
            "manifest_sha256",
            "toolchain",
            "identity_files",
            "build_inherited_environment",
            "training_inherited_environment",
            "instrumented_artifact",
            "raw_profiles",
            "merged_profile",
            "training_ids",
            "profile_generate_flag",
            "receipt_sha256",
        },
        "profile receipt",
    )
    if value["schema"] != PROFILE_RECEIPT_SCHEMA or value["schema_version"] != 1:
        raise PgoError("unsupported profile receipt schema")
    identifier_value(value["profile_id"], "profile receipt profile_id")
    string_value(value["target"], "profile receipt target")
    string_value(value["manifest_sha256"], "profile receipt manifest digest")
    tools = object_value(value["toolchain"], "profile receipt toolchain")
    exact_keys(tools, {"cargo", "rustc", "llvm_profdata"}, "profile receipt toolchain")
    object_value(tools["cargo"], "profile receipt Cargo identity")
    object_value(tools["rustc"], "profile receipt rustc identity")
    object_value(tools["llvm_profdata"], "profile receipt llvm-profdata identity")
    if not isinstance(value["identity_files"], list):
        raise PgoError("profile receipt identity_files must be an array")
    if not isinstance(value["build_inherited_environment"], list):
        raise PgoError("profile receipt build environment must be an array")
    if not isinstance(value["training_ids"], list):
        raise PgoError("profile receipt training_ids must be an array")
    object_value(value["merged_profile"], "profile receipt merged profile")
    claimed = value["receipt_sha256"]
    if not isinstance(claimed, str):
        raise PgoError("profile receipt digest must be a string")
    body = dict(value)
    del body["receipt_sha256"]
    if sha256_bytes(canonical_json(body)) != claimed:
        raise PgoError("profile receipt self-digest mismatch")
    if digest != sha256_bytes(canonical_json(value)) or size != len(canonical_json(value)):
        raise PgoError("profile receipt is not canonically encoded")
    return value


def validate_profile(manifest: Manifest, output: Path) -> dict[str, Any]:
    receipt = load_profile_receipt(output / "profile-receipt.json")
    if receipt["profile_id"] != manifest.profile_id or receipt["target"] != manifest.target:
        raise PgoError("profile id or target does not match the manifest")
    if receipt["manifest_sha256"] != manifest.digest:
        raise PgoError("profile was trained from a different manifest")
    current_tools = toolchain_identity(manifest)
    retained_compiler = {
        "cargo": receipt["toolchain"]["cargo"],
        "rustc": receipt["toolchain"]["rustc"],
    }
    if retained_compiler != current_tools:
        raise PgoError("profile compiler identity differs from the current compiler")
    require_identity_stable(manifest, receipt["identity_files"])
    if receipt["build_inherited_environment"] != inherited_environment_identity(
        manifest.build.inherit_environment
    ):
        raise PgoError("profile build environment differs from the training environment")
    merged_sha, merged_size = stable_file(output / "merged.profdata", "merged profile", MAX_PROFILE_BYTES)
    if receipt["merged_profile"] != {"sha256": merged_sha, "size": merged_size}:
        raise PgoError("merged profile bytes do not match the training receipt")
    if receipt["training_ids"] != [route.identifier for route in manifest.training]:
        raise PgoError("training corpus does not match the profile receipt")
    require_manifest_stable(manifest)
    return receipt


def optimize(manifest: Manifest, output: Path, timeout: int) -> dict[str, Any]:
    profile = validate_profile(manifest, output)
    target_dir = output / "profile-use-target"
    if target_dir.exists():
        raise PgoError(f"profile-use target already exists: {target_dir}")
    artifact = cargo_build(
        manifest,
        output,
        target_dir,
        f"-Cprofile-use={output / 'merged.profdata'}",
        True,
        "profile-use-build.log",
        timeout,
    )
    require_manifest_stable(manifest)
    require_identity_stable(manifest, profile["identity_files"])
    artifact_sha, artifact_size = stable_file(artifact, "profile-use artifact", MAX_ARTIFACT_BYTES)
    body = {
        "schema": BUILD_RECEIPT_SCHEMA,
        "schema_version": 1,
        "mode": "profile_use",
        "profile_id": manifest.profile_id,
        "target": manifest.target,
        "manifest_sha256": manifest.digest,
        "profile_receipt_sha256": profile["receipt_sha256"],
        "merged_profile_sha256": profile["merged_profile"]["sha256"],
        "identity_files": profile["identity_files"],
        "build_inherited_environment": profile["build_inherited_environment"],
        "artifact": {"sha256": artifact_sha, "size": artifact_size},
        "missing_function_warning": True,
    }
    receipt = add_receipt_digest(body)
    atomic_json(output / "profile-use-build-receipt.json", receipt)
    return receipt


def ordinary(manifest: Manifest, output: Path, timeout: int) -> dict[str, Any]:
    if output.exists():
        if not output.is_dir():
            raise PgoError(f"output exists and is not a directory: {output}")
    else:
        output.mkdir(parents=True)
    logs = output / "logs"
    logs.mkdir(exist_ok=True)
    target_dir = output / "ordinary-target"
    if target_dir.exists():
        raise PgoError(f"ordinary target already exists: {target_dir}")
    identities = identity_snapshot(manifest)
    build_inherited_environment = inherited_environment_identity(
        manifest.build.inherit_environment
    )
    tools = toolchain_identity(manifest)
    artifact = cargo_build(
        manifest,
        output,
        target_dir,
        None,
        False,
        "ordinary-build.log",
        timeout,
    )
    require_manifest_stable(manifest)
    require_identity_stable(manifest, identities)
    artifact_sha, artifact_size = stable_file(artifact, "ordinary artifact", MAX_ARTIFACT_BYTES)
    body = {
        "schema": BUILD_RECEIPT_SCHEMA,
        "schema_version": 1,
        "mode": "ordinary",
        "profile_id": manifest.profile_id,
        "target": manifest.target,
        "manifest_sha256": manifest.digest,
        "profile_receipt_sha256": None,
        "merged_profile_sha256": None,
        "toolchain": tools,
        "identity_files": identities,
        "build_inherited_environment": build_inherited_environment,
        "artifact": {"sha256": artifact_sha, "size": artifact_size},
    }
    receipt = add_receipt_digest(body)
    atomic_json(output / "ordinary-build-receipt.json", receipt)
    return receipt


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument("command", choices=("train", "optimize", "all", "ordinary", "verify-profile"))
    result.add_argument("--manifest", required=True, type=Path)
    result.add_argument("--output-dir", required=True, type=Path)
    result.add_argument("--llvm-profdata")
    result.add_argument("--timeout-seconds", type=int, default=3600)
    return result


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        if args.timeout_seconds <= 0:
            raise PgoError("timeout-seconds must be positive")
        output = check_output_location(args.output_dir)
        manifest = load_manifest(args.manifest)
        llvm_profdata = None
        if args.command in {"train", "all"}:
            if args.llvm_profdata is None:
                raise PgoError("--llvm-profdata is required for train and all")
            llvm_profdata = llvm_profdata_path(args.llvm_profdata)
        if args.command in {"train", "all"}:
            assert llvm_profdata is not None
            profile = train(manifest, output, llvm_profdata, args.timeout_seconds)
            print(f"pgo-release: trained profile={manifest.profile_id} receipt_sha256={profile['receipt_sha256']}")
        if args.command in {"optimize", "all"}:
            build = optimize(manifest, output, args.timeout_seconds)
            print(f"pgo-release: built profile-use artifact receipt_sha256={build['receipt_sha256']}")
        elif args.command == "ordinary":
            build = ordinary(manifest, output, args.timeout_seconds)
            print(f"pgo-release: built ordinary artifact receipt_sha256={build['receipt_sha256']}")
        elif args.command == "verify-profile":
            profile = validate_profile(manifest, output)
            print(f"pgo-release: verified profile={manifest.profile_id} receipt_sha256={profile['receipt_sha256']}")
        return 0
    except (PgoError, OSError) as error:
        print(f"pgo-release: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
