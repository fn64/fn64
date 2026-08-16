#!/usr/bin/env python3
"""Qualify RT64 HLSL inputs and produce reviewed SPIR-V artifacts.

This tool is deliberately outside Cargo's ordinary build graph.  Its source-
build and artifact commands are explicit maintenance operations over clean,
pinned checkouts; normal fn64 builds must consume only a reviewed artifact set.
"""

from __future__ import annotations

import argparse
import copy
import hashlib
import json
import os
import posixpath
import re
import shlex
import shutil
import stat
import subprocess
import sys
import tempfile
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path, PurePosixPath


ROOT = Path(__file__).resolve().parent.parent
POLICY_PATH = ROOT / "docs/rt64-shader-artifact-schema.json"
INVENTORY_PATH = ROOT / "docs/rt64-port-inventory.json"
DENOMINATOR_PATH = ROOT / "docs/rt64-shader-source-denominator.json"
REPORT_PATH = ROOT / "docs/RT64-SHADER-ARTIFACTS.md"
TOOL_PATH = Path(__file__).resolve()

POLICY_SCHEMA = "fn64.rt64-shader-artifact-policy.v1"
DENOMINATOR_SCHEMA = "fn64.rt64-shader-source-denominator.v1"
BUILD_SCHEMA = "fn64.dxc-source-build.v2"
VALIDATOR_BUILD_SCHEMA = "fn64.wgpu-shader-validator-build.v1"
RECEIPT_SCHEMA = "fn64.rt64-shader-artifact-receipt.v2"
VALIDATOR_SCHEMA = "fn64.wgpu-shader-validator.v1"
DEPENDENCY_SCHEMA = "fn64.dxc-active-include-closure.v2"
SPIRV_MAGIC = b"\x03\x02\x23\x07"

CALL_NAMES = {
    "preprocess_shader",
    "build_library_shader",
    "build_pixel_shader",
    "build_vertex_shader",
    "build_pixel_shader_spec_constants",
    "build_vertex_shader_spec_constants",
    "build_compute_shader",
    "build_ray_shader",
}
CALL_RE = re.compile(r"^\s*(build_[A-Za-z0-9_]+|preprocess_shader)\s*\((.*)\)\s*$")
OPTION_RE = re.compile(r"^\s*set\s*\(\s*(DXC_(?:COMMON|SPV|PS|VS|CS)_OPTS)\s+(.*?)\)\s*$")
INCLUDE_RE = re.compile(r'^\s*#\s*include\s*([<"])([^>"]+)[>"]', re.MULTILINE)
INCLUDE_DIRECTIVE_RE = re.compile(r"^\s*#\s*include\b", re.MULTILINE)
LOCAL_PATH_RE = re.compile(r"(?:/Users/|/home/|/private/|/tmp/|/var/folders/|[A-Za-z]:\\\\)")
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
GIT_EXECUTABLE = Path(shutil.which("git") or "/usr/bin/git").resolve()

STAGE_FOR_CALL = {
    "build_compute_shader": "compute",
    "build_pixel_shader": "fragment",
    "build_pixel_shader_spec_constants": "fragment",
    "build_vertex_shader": "vertex",
    "build_vertex_shader_spec_constants": "vertex",
}


class ArtifactError(RuntimeError):
    pass


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ArtifactError(message)


def require_keys(value: object, keys: set[str], label: str) -> None:
    require(isinstance(value, dict), f"{label} must be an object")
    actual = set(value)
    require(actual == keys, f"{label} fields changed: {sorted(actual ^ keys)}")


def canonical_json(value: object) -> bytes:
    return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()


def pretty_json(value: object) -> bytes:
    return (json.dumps(value, indent=2, sort_keys=True) + "\n").encode()


def digest_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def digest_file(path: Path, maximum: int | None = None) -> str:
    info = path.lstat()
    require(stat.S_ISREG(info.st_mode), f"not a regular file: {path}")
    require(not path.is_symlink(), f"symlink is not admitted: {path}")
    if maximum is not None:
        require(info.st_size <= maximum, f"file exceeds {maximum} bytes: {path}")
    return digest_bytes(path.read_bytes())


def stable_file_bytes(path: Path, maximum: int, label: str) -> bytes:
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(path, flags)
    except OSError as error:
        raise ArtifactError(f"cannot open {label}: {error}") from error
    try:
        before = os.fstat(descriptor)
        require(stat.S_ISREG(before.st_mode), f"{label} is not a regular file")
        require(before.st_size <= maximum, f"{label} exceeds {maximum} bytes")
        chunks = []
        remaining = maximum + 1
        while remaining:
            chunk = os.read(descriptor, min(1024 * 1024, remaining))
            if not chunk:
                break
            chunks.append(chunk)
            remaining -= len(chunk)
        data = b"".join(chunks)
        after = os.fstat(descriptor)
        identity = lambda value: (
            value.st_dev, value.st_ino, value.st_mode, value.st_nlink, value.st_size,
            value.st_mtime_ns, value.st_ctime_ns,
        )
        require(identity(before) == identity(after), f"{label} changed while it was read")
        require(len(data) == before.st_size, f"{label} length changed while it was read")
        require(len(data) <= maximum, f"{label} exceeds {maximum} bytes")
        return data
    finally:
        os.close(descriptor)


def file_identity(info: os.stat_result) -> tuple[int, int, int, int, int, int, int]:
    return (
        info.st_dev,
        info.st_ino,
        info.st_mode,
        info.st_nlink,
        info.st_size,
        info.st_mtime_ns,
        info.st_ctime_ns,
    )


def stable_regular_bytes(path: Path, maximum: int, label: str) -> tuple[bytes, os.stat_result]:
    before = path.lstat()
    require(stat.S_ISREG(before.st_mode) and not path.is_symlink(), f"{label} is not a regular file")
    data = stable_file_bytes(path, maximum, label)
    after = path.lstat()
    require(file_identity(before) == file_identity(after), f"{label} path changed while it was read")
    return data, after


def contained_parent_identities(root: Path, path: Path, label: str) -> list[tuple[Path, tuple[int, int, int, int, int, int, int]]]:
    require(root.is_absolute() and path.is_absolute(), f"{label} containment requires absolute paths")
    relative = relative_to(path, root)
    require(relative is not None and relative.parts, f"{label} escaped its admitted root")
    root_info = root.lstat()
    require(stat.S_ISDIR(root_info.st_mode) and not root.is_symlink(), f"{label} root is not a regular directory")
    rows = [(root, file_identity(root_info))]
    current = root
    for part in relative.parts[:-1]:
        current /= part
        info = current.lstat()
        require(stat.S_ISDIR(info.st_mode) and not current.is_symlink(), f"{label} has a non-directory or symlinked parent")
        rows.append((current, file_identity(info)))
    return rows


def verify_parent_identities(
    rows: list[tuple[Path, tuple[int, int, int, int, int, int, int]]],
    label: str,
) -> None:
    for path, expected in rows:
        info = path.lstat()
        require(
            stat.S_ISDIR(info.st_mode)
            and not path.is_symlink()
            and file_identity(info) == expected,
            f"{label} parent path changed while it was qualified",
        )


@dataclass(frozen=True)
class ContainedExecutable:
    root: Path
    invocation_path: Path
    target_path: Path
    receipt_record: dict


def qualify_contained_executable(root: Path, invocation: Path, maximum: int, label: str) -> ContainedExecutable:
    """Qualify one executable leaf without resolving away an admitted symlink edge."""
    root = root.absolute()
    invocation = invocation.absolute()
    parents = contained_parent_identities(root, invocation, label)
    alias_before = invocation.lstat()
    require(alias_before.st_nlink == 1, f"{label} invocation path has another hardlink")

    if stat.S_ISLNK(alias_before.st_mode):
        link_text = os.readlink(invocation)
        require(link_text and not os.path.isabs(link_text), f"{label} symlink target must be relative")
        link_parts = Path(link_text).parts
        require(".." not in link_parts, f"{label} symlink target may not escape with '..'")
        target = invocation.parent.joinpath(*link_parts).absolute()
        require(relative_to(target, root) is not None and target != invocation, f"{label} symlink target escaped its admitted root")
        target_parents = contained_parent_identities(root, target, f"{label} target")
        target_before = target.lstat()
        require(not stat.S_ISLNK(target_before.st_mode), f"{label} symlink target is another symlink")
        require(stat.S_ISREG(target_before.st_mode), f"{label} symlink target is not a regular file")
        require(target_before.st_nlink == 1, f"{label} target has another hardlink")
        require(bool(target_before.st_mode & 0o111), f"{label} target is not executable")
        data, target_after = stable_regular_bytes(target, maximum, f"{label} target")
        alias_after = invocation.lstat()
        require(
            file_identity(alias_before) == file_identity(alias_after)
            and stat.S_ISLNK(alias_after.st_mode)
            and os.readlink(invocation) == link_text,
            f"{label} symlink changed while its target was read",
        )
        require(file_identity(target_before) == file_identity(target_after), f"{label} target path changed while it was read")
        verify_parent_identities(parents, label)
        verify_parent_identities(target_parents, f"{label} target")
        record = {
            "kind": "relative-contained-symlink",
            "invocation_relative_path": invocation.relative_to(root).as_posix(),
            "link_text": link_text,
            "target_relative_path": target.relative_to(root).as_posix(),
            "target_bytes": len(data),
            "target_sha256": digest_bytes(data),
        }
        return ContainedExecutable(root, invocation, target, record)

    require(stat.S_ISREG(alias_before.st_mode), f"{label} invocation path is not a regular file or symlink")
    require(bool(alias_before.st_mode & 0o111), f"{label} invocation path is not executable")
    data, alias_after = stable_regular_bytes(invocation, maximum, label)
    require(file_identity(alias_before) == file_identity(alias_after), f"{label} path changed while it was read")
    verify_parent_identities(parents, label)
    record = {
        "kind": "regular",
        "invocation_relative_path": invocation.relative_to(root).as_posix(),
        "target_bytes": len(data),
        "target_sha256": digest_bytes(data),
    }
    return ContainedExecutable(root, invocation, invocation, record)


def write_new_private_file(path: Path, data: bytes) -> None:
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(path, flags, 0o600)
    except OSError as error:
        raise ArtifactError(f"cannot create private staged file {path.name}: {error}") from error
    try:
        view = memoryview(data)
        while view:
            written = os.write(descriptor, view)
            require(written > 0, f"short write while staging {path.name}")
            view = view[written:]
        os.fsync(descriptor)
        os.fchmod(descriptor, 0o400)
    finally:
        os.close(descriptor)
    info = path.lstat()
    require(stat.S_ISREG(info.st_mode) and not path.is_symlink(), f"staged file is not regular: {path.name}")
    require(info.st_nlink == 1, f"staged file has another link: {path.name}")


def source_snapshot_record(denominator: dict) -> dict:
    files = [
        {"path": row["path"], "sha256": row["port_sha256"]}
        for row in denominator["source_files"]
    ]
    return {
        "schema": "fn64.rt64-shader-staged-source.v1",
        "copy_mode": "descriptor-stable-create-new-no-links-private",
        "files": files,
        "source_set_sha256": digest_bytes(canonical_json(files)),
    }


def stage_rt64_source_snapshot(port: Path, stage: Path, denominator: dict, maximum: int) -> dict:
    stage.mkdir(mode=0o700)
    expected = source_snapshot_record(denominator)
    for row in expected["files"]:
        source = port.joinpath(*PurePosixPath(row["path"]).parts)
        data = stable_file_bytes(source, maximum, f"RT64 source {row['path']}")
        require(digest_bytes(data) == row["sha256"], f"RT64 source changed before staging: {row['path']}")
        destination = stage.joinpath(*PurePosixPath(row["path"]).parts)
        write_new_private_file(destination, data)
        copied = stable_file_bytes(destination, maximum, f"staged RT64 source {row['path']}")
        require(digest_bytes(copied) == row["sha256"], f"staged RT64 source digest mismatch: {row['path']}")
    return expected


def verify_staged_dependencies(stage: Path, paths: list[str], denominator: dict, maximum: int) -> list[dict]:
    expected = {row["path"]: row["port_sha256"] for row in denominator["source_files"]}
    rows = []
    for relative in sorted(paths):
        require(relative in expected, f"staged dependency is outside the denominator: {relative}")
        path = stage.joinpath(*PurePosixPath(relative).parts)
        info = path.lstat()
        require(info.st_nlink == 1, f"staged dependency gained another link: {relative}")
        data = stable_file_bytes(path, maximum, f"staged dependency {relative}")
        actual = digest_bytes(data)
        require(actual == expected[relative], f"staged dependency bytes changed: {relative}")
        rows.append({"path": relative, "sha256": actual})
    return rows


def load_json(path: Path) -> dict:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ArtifactError(f"cannot load {path}: {error}") from error
    require(isinstance(value, dict), f"{path} must contain a JSON object")
    return value


def load_canonical_json(path: Path, maximum: int, label: str) -> dict:
    digest_file(path, maximum)
    value = load_json(path)
    require(path.read_bytes() == pretty_json(value), f"{label} JSON bytes are not canonical")
    return value


def git(directory: Path, *arguments: str, check: bool = True) -> str:
    try:
        result = subprocess.run(
            [
                str(GIT_EXECUTABLE), "--no-replace-objects",
                "-c", "core.fsmonitor=false",
                "-c", "core.untrackedCache=false",
                "-C", str(directory), *arguments,
            ],
            check=check,
            capture_output=True,
            text=True,
            env={
                "PATH": str(GIT_EXECUTABLE.parent),
                "LC_ALL": "C",
                "LANG": "C",
                "GIT_CONFIG_NOSYSTEM": "1",
                "GIT_CONFIG_GLOBAL": os.devnull,
                "GIT_OPTIONAL_LOCKS": "0",
                "GIT_TERMINAL_PROMPT": "0",
            },
        )
    except (OSError, subprocess.CalledProcessError) as error:
        detail = getattr(error, "stderr", "") or str(error)
        raise ArtifactError(f"git {' '.join(arguments)} failed: {detail.strip()}") from error
    return result.stdout.strip()


def git_raw(directory: Path, *arguments: str) -> bytes:
    try:
        return subprocess.run(
            [
                str(GIT_EXECUTABLE), "--no-replace-objects",
                "-c", "core.fsmonitor=false",
                "-c", "core.untrackedCache=false",
                "-C", str(directory), *arguments,
            ],
            check=True,
            capture_output=True,
            env={
                "PATH": str(GIT_EXECUTABLE.parent),
                "LC_ALL": "C",
                "LANG": "C",
                "GIT_CONFIG_NOSYSTEM": "1",
                "GIT_CONFIG_GLOBAL": os.devnull,
                "GIT_OPTIONAL_LOCKS": "0",
                "GIT_TERMINAL_PROMPT": "0",
            },
        ).stdout
    except (OSError, subprocess.CalledProcessError) as error:
        detail = getattr(error, "stderr", b"")
        if isinstance(detail, bytes):
            detail = detail.decode(errors="replace")
        raise ArtifactError(f"git {' '.join(arguments)} failed: {detail.strip()}") from error


def parse_git_tree_rows(payload: bytes, index: bool) -> dict[str, tuple[str, str]]:
    rows: dict[str, tuple[str, str]] = {}
    for raw in payload.split(b"\0"):
        if not raw:
            continue
        metadata, separator, path_bytes = raw.partition(b"\t")
        require(separator == b"\t", "malformed Git tree/index row")
        try:
            path = path_bytes.decode("utf-8")
            fields = metadata.decode("ascii").split()
        except UnicodeDecodeError as error:
            raise ArtifactError("Git tree/index row is not UTF-8/ASCII") from error
        if index:
            require(len(fields) == 3 and fields[2] == "0", f"unmerged Git index row: {path}")
            mode, object_id = fields[:2]
        else:
            require(len(fields) == 3, f"malformed Git tree row: {path}")
            mode, _, object_id = fields
        require(path not in rows, f"duplicate Git tree/index path: {path}")
        rows[path] = (mode, object_id)
    return rows


def git_blob_object_id(data: bytes, algorithm: str) -> str:
    require(algorithm in {"sha1", "sha256"}, f"unsupported Git object format: {algorithm}")
    digest = hashlib.new(algorithm)
    digest.update(f"blob {len(data)}\0".encode("ascii"))
    digest.update(data)
    return digest.hexdigest()


def validate_pinned_git_worktree(
    tree: Path,
    commit: str,
    label: str,
    allow_skip_worktree: bool,
    maximum_file_bytes: int,
    verify_materialized_bytes: bool = True,
) -> None:
    top_level = Path(git(tree, "rev-parse", "--show-toplevel")).resolve()
    require(top_level == tree.resolve(), f"{label} Git worktree root is redirected")
    replace_refs = git(tree, "for-each-ref", "--format=%(refname)", "refs/replace/").splitlines()
    require(not replace_refs, f"{label} has replacement refs: {replace_refs}")
    common_directory = Path(git(tree, "rev-parse", "--git-common-dir"))
    if not common_directory.is_absolute():
        common_directory = tree / common_directory
    require(not (common_directory.resolve() / "info/grafts").exists(), f"{label} has a legacy graft file")
    committed = parse_git_tree_rows(git_raw(tree, "ls-tree", "-r", "-z", commit), index=False)
    indexed = parse_git_tree_rows(git_raw(tree, "ls-files", "--stage", "-z"), index=True)
    require(indexed == committed, f"{label} Git index blob/mode/tree differs from the pinned commit")
    markers: dict[str, str] = {}
    for raw in git_raw(tree, "ls-files", "-v", "-z").split(b"\0"):
        if not raw:
            continue
        require(len(raw) >= 3 and raw[1:2] == b" ", f"malformed {label} Git index-mask row")
        try:
            marker = raw[:1].decode("ascii")
            path = raw[2:].decode("utf-8")
        except UnicodeDecodeError as error:
            raise ArtifactError(f"{label} Git index-mask row is not UTF-8/ASCII") from error
        require(path not in markers, f"duplicate {label} Git index-mask path: {path}")
        markers[path] = marker
    require(set(markers) == set(committed), f"{label} Git index-mask denominator differs from the pinned tree")
    algorithm = git(tree, "rev-parse", "--show-object-format")
    for relative, (mode, object_id) in committed.items():
        marker = markers[relative]
        if marker == "S" and allow_skip_worktree:
            continue
        require(marker == "H", f"{label} has an index mask on {relative}: {marker}")
        if not verify_materialized_bytes:
            continue
        path = tree.joinpath(*PurePosixPath(relative).parts)
        if mode == "160000":
            require(path.is_dir(), f"{label} gitlink is not materialized: {relative}")
            continue
        if mode == "120000":
            require(path.is_symlink(), f"{label} symlink mode changed: {relative}")
            data = os.fsencode(os.readlink(path))
        else:
            require(mode in {"100644", "100755"}, f"{label} has an unsupported Git mode {mode}: {relative}")
            data = stable_file_bytes(path, maximum_file_bytes, f"{label} tracked file {relative}")
            executable_mode = bool(path.lstat().st_mode & 0o111)
            require(executable_mode == (mode == "100755"), f"{label} executable mode changed: {relative}")
        require(git_blob_object_id(data, algorithm) == object_id, f"{label} working bytes differ from pinned Git blob: {relative}")


def validate_complete_pinned_git_worktree(
    tree: Path,
    commit: str,
    label: str,
    maximum_file_bytes: int,
) -> None:
    sparse = git(tree, "config", "--bool", "core.sparseCheckout", check=False)
    require(sparse != "true", f"{label} requires a complete, non-sparse checkout")
    validate_pinned_git_worktree(
        tree,
        commit,
        label,
        allow_skip_worktree=False,
        maximum_file_bytes=maximum_file_bytes,
    )


def load_policy() -> dict:
    policy = load_json(POLICY_PATH)
    require(policy.get("schema") == POLICY_SCHEMA, "unsupported shader artifact policy")
    require(policy.get("receipt_schema") == RECEIPT_SCHEMA, "receipt schema drift")
    require(policy.get("source_denominator_schema") == DENOMINATOR_SCHEMA, "denominator schema drift")
    require(policy.get("build_receipt_schema") == BUILD_SCHEMA, "build schema drift")
    return policy


def load_inventory(policy: dict) -> dict:
    inventory = load_json(INVENTORY_PATH)
    require(inventory.get("schema") == "fn64.rt64-port-inventory.v2", "unsupported RT64 port inventory")
    sources = inventory.get("sources", {})
    require(sources.get("oracle", {}).get("commit") == policy["rt64"]["oracle_commit"], "oracle pin drift")
    require(sources.get("port", {}).get("commit") == policy["rt64"]["port_commit"], "port pin drift")
    return inventory


def validate_clean_rt64_tree(tree: Path, expected_commit: str, label: str) -> None:
    require(tree.is_dir(), f"{label} RT64 checkout does not exist")
    require(git(tree, "rev-parse", "HEAD") == expected_commit, f"{label} RT64 checkout is at the wrong pin")
    require(
        not git(tree, "status", "--porcelain", "--untracked-files=all", "--ignore-submodules=none"),
        f"{label} RT64 checkout is dirty",
    )
    validate_pinned_git_worktree(
        tree,
        expected_commit,
        f"{label} RT64 checkout",
        allow_skip_worktree=False,
        maximum_file_bytes=load_policy()["git_checkout_maximum_file_bytes"],
    )


def git_blob(tree: Path, commit: str, path: str) -> bytes:
    try:
        return git_raw(tree, "show", f"{commit}:{path}")
    except ArtifactError as error:
        raise ArtifactError(f"cannot read git:{commit}:{path}: {error}") from error


def inventory_files(inventory: dict) -> dict[str, dict]:
    files = inventory.get("files")
    require(isinstance(files, list), "inventory files must be an array")
    indexed: dict[str, dict] = {}
    for item in files:
        require(isinstance(item, dict) and isinstance(item.get("path"), str), "malformed inventory file row")
        path = item["path"]
        require(path not in indexed, f"duplicate inventory path: {path}")
        indexed[path] = item
    return indexed


def split_cmake_argument(argument: str) -> list[str]:
    try:
        return shlex.split(argument)
    except ValueError as error:
        raise ArtifactError(f"malformed CMake shader option {argument!r}: {error}") from error


def parse_cmake_shader_calls(cmake: bytes) -> list[dict]:
    try:
        text = cmake.decode("utf-8")
    except UnicodeDecodeError as error:
        raise ArtifactError("RT64 CMakeLists.txt is not UTF-8") from error
    calls: list[dict] = []
    for line_number, line in enumerate(text.splitlines(), 1):
        match = CALL_RE.match(line)
        if match is None:
            continue
        name = match.group(1)
        try:
            arguments = shlex.split(match.group(2))
        except ValueError as error:
            raise ArtifactError(f"malformed CMake shader call at line {line_number}: {error}") from error
        if not arguments or arguments[0] != "rt64":
            continue
        require(name in CALL_NAMES, f"unclassified RT64 shader call {name} at CMake line {line_number}")
        require(len(arguments) >= 2, f"shader call at CMake line {line_number} lacks a source")
        calls.append({"line": line_number, "function": name, "arguments": arguments[1:]})
    require(calls, "no RT64 shader calls were found")
    return calls


def parse_cmake_shader_options(cmake: bytes) -> dict[str, list[str]]:
    try:
        text = cmake.decode("utf-8")
    except UnicodeDecodeError as error:
        raise ArtifactError("RT64 CMakeLists.txt is not UTF-8") from error
    values: dict[str, list[str]] = {}
    for line in text.splitlines():
        match = OPTION_RE.match(line)
        if match is None:
            continue
        require(match.group(1) not in values, f"duplicate RT64 CMake shader option {match.group(1)}")
        arguments = shlex.split(match.group(2))
        flattened = []
        for argument in arguments:
            flattened.extend(split_cmake_argument(argument))
        values[match.group(1)] = flattened
    expected_names = {"DXC_COMMON_OPTS", "DXC_SPV_OPTS", "DXC_PS_OPTS", "DXC_VS_OPTS", "DXC_CS_OPTS"}
    require(set(values) == expected_names, f"RT64 CMake shader option denominator changed: {sorted(set(values) ^ expected_names)}")
    return values


def validate_cmake_shader_options(options: dict[str, list[str]], policy: dict) -> None:
    require(options["DXC_COMMON_OPTS"] == ["-I${PROJECT_SOURCE_DIR}/src"], "RT64 common shader flags changed")
    require(options["DXC_SPV_OPTS"] == policy["spirv"]["common_flags"][:3], "RT64 SPIR-V flags changed")
    common = ["${DXC_COMMON_OPTS}"]
    for stage, variable in (("fragment", "DXC_PS_OPTS"), ("vertex", "DXC_VS_OPTS"), ("compute", "DXC_CS_OPTS")):
        profile = policy["spirv"]["profiles"][stage]
        expected = common + ["-E", profile["entry"], "-T", profile["profile"], *profile["extra_flags"]]
        require(options[variable] == expected, f"RT64 {stage} shader flags changed")


def compilation_row(call: dict, policy: dict) -> dict | None:
    function = call["function"]
    arguments = call["arguments"]
    if function in {"preprocess_shader", "build_library_shader"}:
        return None
    require(function in STAGE_FOR_CALL, f"no SPIR-V stage mapping for {function}")
    source = arguments[0]
    require(source.endswith(".hlsl"), f"SPIR-V entry source is not HLSL: {source}")
    output = arguments[1] if len(arguments) > 1 and not arguments[1].startswith("-") else source
    extra_start = 2 if output != source or (len(arguments) > 1 and arguments[1] == source) else 1
    extras: list[str] = []
    for argument in arguments[extra_start:]:
        extras.extend(split_cmake_argument(argument))
    stage = STAGE_FOR_CALL[function]
    stage_policy = policy["spirv"]["profiles"][stage]
    flags = list(policy["spirv"]["common_flags"])
    flags += ["-E", stage_policy["entry"], "-T", stage_policy["profile"]]
    flags += stage_policy["extra_flags"]
    flags += extras
    require(flags.count("-spirv") == 1 and "-Vd" not in flags and "/Vd" not in flags, "SPIR-V validation flags are not fail closed")
    stem = output.removesuffix(".hlsl")
    identifier = re.sub(r"[^a-z0-9]+", "-", stem.lower()).strip("-")
    return {
        "id": identifier,
        "port_cmake_line": call["line"],
        "cmake_function": function,
        "source": source,
        "output_name": output,
        "stage": stage,
        "entry": stage_policy["entry"],
        "profile": stage_policy["profile"],
        "flags": flags,
        "preprocessed_artifact": f"preprocessed/{output}.pp.hlsl",
        "dependency_manifest_artifact": f"dependencies/{output}.json",
        "spirv_artifact": f"spirv/{output}.spv",
    }


def resolve_include(source: str, include: str, files: dict[str, dict]) -> str | None:
    candidates = [
        posixpath.normpath(str(PurePosixPath(source).parent / include)),
        posixpath.normpath(str(PurePosixPath("src") / include)),
        posixpath.normpath(include),
    ]
    for candidate in candidates:
        if candidate in files:
            return candidate
    return None


def derive_include_graph(
    port_tree: Path, files: dict[str, dict], shader_paths: set[str], policy: dict
) -> tuple[list[dict], list[dict], list[dict]]:
    allow = {(item["source"], item["include"]): item["reason"] for item in policy["unresolved_include_allowlist"]}
    queue = list(sorted(shader_paths))
    seen: set[str] = set()
    edges: list[dict] = []
    unresolved: list[dict] = []
    while queue:
        source = queue.pop()
        if source in seen:
            continue
        seen.add(source)
        text = (port_tree / source).read_text(encoding="utf-8")
        for match in INCLUDE_RE.finditer(text):
            delimiter, include = match.groups()
            resolved = resolve_include(source, include, files)
            if resolved is not None:
                edges.append({"source": source, "include": include, "resolved": resolved})
                if resolved not in seen:
                    queue.append(resolved)
                continue
            if delimiter == "<":
                unresolved.append({
                    "source": source,
                    "include": include,
                    "classification": "system-or-cpp-only",
                })
                continue
            reason = allow.get((source, include))
            require(reason is not None, f"unresolved quoted include is not reviewed: {source} -> {include}")
            unresolved.append({
                "source": source,
                "include": include,
                "classification": "reviewed-unresolved",
                "reason": reason,
            })
    edges.sort(key=lambda item: (item["source"], item["include"], item["resolved"]))
    unresolved.sort(key=lambda item: (item["source"], item["include"]))
    source_rows = []
    for path in sorted(seen):
        row = files[path]
        actual = digest_file(port_tree / path, policy["spirv"]["maximum_source_bytes"])
        expected = row["sources"]["port"]["sha256"]
        require(actual == expected, f"port source digest drift: {path}")
        source_rows.append({
            "path": path,
            "port_sha256": expected,
            "oracle_sha256": row["sources"]["oracle"]["sha256"],
            "port_delta": row["port_delta"],
        })
    return source_rows, edges, unresolved


def transitive_dependencies(source: str, edges: list[dict]) -> list[str]:
    outgoing: dict[str, list[str]] = {}
    for edge in edges:
        outgoing.setdefault(edge["source"], []).append(edge["resolved"])
    seen: set[str] = set()
    stack = [source]
    while stack:
        current = stack.pop()
        if current in seen:
            continue
        seen.add(current)
        stack.extend(outgoing.get(current, []))
    return sorted(seen)


def derive_denominator(port_tree: Path, oracle_tree: Path | None = None) -> dict:
    policy = load_policy()
    inventory = load_inventory(policy)
    port_commit = policy["rt64"]["port_commit"]
    oracle_commit = policy["rt64"]["oracle_commit"]
    validate_clean_rt64_tree(port_tree, port_commit, "port")
    if oracle_tree is not None:
        validate_clean_rt64_tree(oracle_tree, oracle_commit, "oracle")

    files = inventory_files(inventory)
    shader_paths = {
        path for path in files if path.startswith("src/shaders/") and Path(path).suffix in {".hlsl", ".hlsli"}
    }
    require(shader_paths, "inventory contains no admitted shader files")
    for path in shader_paths:
        require((port_tree / path).is_file(), f"port shader is missing: {path}")

    port_cmake = (port_tree / "CMakeLists.txt").read_bytes()
    oracle_cmake = (
        (oracle_tree / "CMakeLists.txt").read_bytes()
        if oracle_tree is not None
        else git_blob(port_tree, oracle_commit, "CMakeLists.txt")
    )
    calls = parse_cmake_shader_calls(port_cmake)
    oracle_calls = parse_cmake_shader_calls(oracle_cmake)
    port_options = parse_cmake_shader_options(port_cmake)
    oracle_options = parse_cmake_shader_options(oracle_cmake)
    require(port_options == oracle_options, "dual-pin CMake shader options differ")
    validate_cmake_shader_options(port_options, policy)
    port_semantics = [{"function": call["function"], "arguments": call["arguments"]} for call in calls]
    oracle_semantics = [{"function": call["function"], "arguments": call["arguments"]} for call in oracle_calls]
    require(port_semantics == oracle_semantics, "dual-pin CMake shader call denominator differs")

    entries = []
    for call, oracle_call in zip(calls, oracle_calls, strict=True):
        row = compilation_row(call, policy)
        if row is not None:
            row["oracle_cmake_line"] = oracle_call["line"]
            entries.append(row)
    ids = [entry["id"] for entry in entries]
    require(len(ids) == len(set(ids)), "duplicate SPIR-V artifact identity")
    hlsl_sources = {path for path in shader_paths if path.endswith(".hlsl")}
    compiled_sources = {entry["source"] for entry in entries}
    library_sources = {call["arguments"][0] for call in calls if call["function"] == "build_library_shader"}
    require(hlsl_sources == compiled_sources | library_sources, "admitted HLSL source is absent from the pinned CMake denominator")

    source_rows, include_edges, unresolved = derive_include_graph(port_tree, files, shader_paths, policy)
    for row in source_rows:
        oracle_bytes = (
            (oracle_tree / row["path"]).read_bytes()
            if oracle_tree is not None
            else git_blob(port_tree, oracle_commit, row["path"])
        )
        require(len(oracle_bytes) <= policy["spirv"]["maximum_source_bytes"], f"oracle source exceeds size bound: {row['path']}")
        require(digest_bytes(oracle_bytes) == row["oracle_sha256"], f"oracle source digest drift: {row['path']}")
    source_by_path = {row["path"]: row for row in source_rows}
    for entry in entries:
        dependencies = transitive_dependencies(entry["source"], include_edges)
        entry["dependency_files"] = dependencies
        entry["dependency_set_sha256"] = digest_bytes(canonical_json([
            {"path": path, "sha256": source_by_path[path]["port_sha256"]} for path in dependencies
        ]))

    non_spirv = []
    preprocess_only = []
    for call, oracle_call in zip(calls, oracle_calls, strict=True):
        if call["function"] == "build_library_shader":
            non_spirv.append({
                "port_cmake_line": call["line"],
                "oracle_cmake_line": oracle_call["line"],
                "source": call["arguments"][0],
                "output_name": call["arguments"][1],
                "classification": "windows-dxil-library-only",
            })
        elif call["function"] == "preprocess_shader":
            preprocess_only.append({
                "port_cmake_line": call["line"],
                "oracle_cmake_line": oracle_call["line"],
                "source": call["arguments"][0],
                "classification": "runtime-generated-raster-parameter-text",
            })

    payload = {
        "schema": DENOMINATOR_SCHEMA,
        "generated_by": "tools/rt64_shader_artifacts.py",
        "authority": {
            "repository": policy["rt64"]["repository"],
            "oracle_commit": oracle_commit,
            "port_commit": port_commit,
            "shader_sources_identical": all(row["port_sha256"] == row["oracle_sha256"] for row in source_rows if row["path"].startswith("src/shaders/")),
            "dual_pin_source_set_verified": True,
            "port_source_set_sha256": digest_bytes(canonical_json([
                {"path": row["path"], "sha256": row["port_sha256"]} for row in source_rows
            ])),
            "oracle_source_set_sha256": digest_bytes(canonical_json([
                {"path": row["path"], "sha256": row["oracle_sha256"]} for row in source_rows
            ])),
            "oracle_cmake_sha256": digest_bytes(oracle_cmake),
            "port_cmake_sha256": digest_bytes(port_cmake),
            "port_inventory_sha256": digest_file(INVENTORY_PATH),
            "cmake_shader_options": port_options,
        },
        "counts": {
            "spirv_entries": len(entries),
            "hlsl_sources": len(hlsl_sources),
            "hlsli_sources": sum(path.endswith(".hlsli") for path in shader_paths),
            "dependency_files": len(source_rows),
            "include_edges": len(include_edges),
            "non_spirv_entries": len(non_spirv),
            "preprocess_only": len(preprocess_only),
        },
        "entries": entries,
        "non_spirv_entries": non_spirv,
        "preprocess_only": preprocess_only,
        "source_files": source_rows,
        "include_edges": include_edges,
        "unresolved_includes": unresolved,
    }
    payload["denominator_sha256"] = digest_bytes(canonical_json(payload))
    require(not LOCAL_PATH_RE.search(json.dumps(payload)), "source denominator leaked a machine-local path")
    return payload


def render_report(denominator: dict, dxc_audit: dict | None = None) -> str:
    counts = denominator["counts"]
    authority = denominator["authority"]
    dxc_status = "policy closure frozen; `audit-dxc` revalidates a local checkout"
    if dxc_audit is not None:
        dxc_status = f"source closure verified at `{dxc_audit['commit'][:12]}`"
    lines = [
        "# RT64 shader artifacts",
        "",
        "<!-- Generated by tools/rt64_shader_artifacts.py. Edit the policy or admitted source, not the counts below. -->",
        "",
        "Status: **source denominator and build mechanisms implemented; artifact corpus not yet qualified.**",
        "",
        "This is the fail-closed maintenance boundary for compiling the admitted RT64 HLSL corpus to reviewed SPIR-V. It is not part of Cargo's ordinary build, and it does not place DXC, CMake, RT64 C++, or preprocessed source in fn64's runtime dependency graph.",
        "",
        "## Frozen authorities",
        "",
        f"- RT64 executable oracle: `{authority['oracle_commit']}`.",
        f"- RT64 semantic port source: `{authority['port_commit']}`.",
        f"- Shader source equality across the pins: `{str(authority['shader_sources_identical']).lower()}`.",
        f"- Complete source/dependency set verified at both pins: `{str(authority['dual_pin_source_set_verified']).lower()}` (`{authority['port_source_set_sha256']}`).",
        "- Official DXC source: `v1.9.2607` / `0d3ee6b551b8fa768fbf825300ebab81047ef6a8`.",
        "- DXC source-license closure: NCSA root plus retained `ThirdPartyNotices.txt`, every tracked in-tree license/notice file, and every tracked license file in the exact DirectX-Headers, SPIRV-Headers, and SPIRV-Tools gitlinks. SPIRV-Headers is file-scoped MIT/CC-BY-4.0/public-domain-or-MIT; compiler-consumed headers are under its MIT material grant.",
        f"- DXC source audit in this report: {dxc_status}.",
        "",
        "The tag's `.gitmodules` names googletest but the tag has no googletest gitlink. The isolated producer disables DXC, HLSL, LLVM, and SPIR-V tests, so it does not fetch or silently admit that stale declaration.",
        "",
        "## Denominator",
        "",
        f"- `{counts['spirv_entries']}` SPIR-V artifact variants from `{counts['hlsl_sources']}` admitted HLSL entry sources.",
        f"- `{counts['hlsli_sources']}` admitted HLSLI sources, `{counts['dependency_files']}` total reachable admitted source files, and `{counts['include_edges']}` declared include edges.",
        f"- `{counts['non_spirv_entries']}` Windows-only DXIL library builds and `{counts['preprocess_only']}` preprocess-only source are named separately rather than omitted.",
        f"- Denominator identity: `{denominator['denominator_sha256']}`.",
        "",
        "`Lights.hlsli` names an absent `Ray.hlsli`, but no pinned CMake shader entry or preprocess target reaches `Lights.hlsli`. This is retained as an explicit M12 source gap; the producer cannot manufacture a ray artifact from it.",
        "",
        "## Qualification contract",
        "",
        "`build-dxc` accepts only a clean, complete, non-sparse official source commit with its exact initialized gitlinks and license bytes. All authority Git commands disable replacement objects and global/system configuration; replacement refs and legacy grafts are rejected. The build rejects index/tree blob or mode disagreement, skip/assume index masks, transformed working-tree bytes, and unreviewed nested gitlinks before invoking the official CMake graph in a new isolated output directory, then repeats the complete materialized audit after configure/build. Receipt verification repeats that same complete audit. Its receipt binds source, dependencies, every retained license, CMake cache/flags, tool binaries and version transcripts, configure/build logs, compile commands, Ninja execution log, the exact digested translation-unit manifest actually executed for the `dxc` target grouped by license component, and the resulting compiler closure. On macOS, the official graph makes `bin/dxc` a relative symlink to its versioned launcher and that launcher loads the retained `libdxcompiler.dylib`; the receipt preserves the invocation edge, binds exact link text and descriptor-stable bytes for both non-system files, and admits only an exact system-library load-name denominator. Absolute, escaping, multi-hop, nonregular, swapped, hardlinked, multiply emitted, or unclassified non-system paths are rejected. Version inspection executes only a private create-new/no-link snapshot with the launcher's `@rpath` layout preserved.",
        "Receipt finalization currently fails closed on non-macOS hosts. A Linux or Windows source-build receipt needs its own reviewed loader format, system-library denominator, retained dependency paths, and hostile tests before this policy will admit it.",
        "",
        "`build-validator` separately stages the locked standalone Rust validator outside fn64's Cargo-config ancestry, invokes Cargo from the configuration-checked filesystem root with a new controlled Cargo home/target, remaps the isolated build root to a stable virtual source path, uses direct cargo/rustc toolchain binaries, and builds wgpu 30.0.0's deterministic noop backend. Because noop does not itself surface shader errors, the validator explicitly runs the same pinned naga parser, all-flags validator, and wgpu-naga-bridge baseline feature capability mapping used by wgpu-core 30 before invoking checked `Device::create_shader_module`. Its receipt binds the reviewed and staged source, Cargo.lock, complete Cargo package/license closure, toolchain, build transcript, binary, and stable protocol identity. This workspace is not in fn64's ordinary Cargo graph.",
        "",
        "`produce` accepts only both source-build receipts. Every receipt also binds the exact artifact-tool source. It descriptor-stably copies the complete admitted RT64 source set and the qualified DXC runtime closure into separate private create-new/no-link snapshots. Each row uses three explicit, non-overlapping compiler phases through that one closure: dependency-only `-M -MF` over the admitted source, preprocess-only `-P -Fi` over the same source, then SPIR-V compilation from only the retained preprocessed bytes. The producer validates the dependency target as the exact relative entry source, retains and hashes DXC's raw depfile plus its normalized active-dependency manifest, checks all declared source bytes after both source-reading phases, and rejects missing, reused, malformed, or unexpected phase outputs. Any `#include` directive left in the retained preprocessed bytes, including a macro-form operand, is rejected because the final compiler flags still carry an include search path. For each denominator row it also retains exact canonical flags, preprocessed-input digest, all phase transcript digests, SPIR-V bytes and digest, DXC's mandatory built-in SPIR-V validation result, and an independently bound wgpu-30 shader-module validation result. `verify` reparses the raw depfile and reruns wgpu validation. One missing row, unexpected row, failed validator, changed byte, changed flag, changed compiler, changed producer, phase confusion, or reused path rejects the set.",
        "",
        "A receipt is local integrity evidence, not a transferable signature or proof against a malicious same-UID process. Release provenance still needs a trusted CI/code-signing root if artifacts are distributed as independently attested builds.",
        "",
        "## Commands",
        "",
        "```sh",
        "python3 tools/rt64_shader_artifacts.py check --port-dir /absolute/path/to/clean/rt64-port --oracle-dir /absolute/path/to/clean/rt64-oracle",
        "python3 tools/rt64_shader_artifacts.py audit-dxc --dxc-dir /absolute/path/to/DirectXShaderCompiler",
        "python3 tools/rt64_shader_artifacts.py build-dxc --dxc-dir /absolute/path/to/DirectXShaderCompiler --output-dir /outside/repo/dxc-build",
        "python3 tools/rt64_shader_artifacts.py verify-dxc-build --dxc-dir /absolute/path/to/DirectXShaderCompiler --build-dir /outside/repo/dxc-build",
        "python3 tools/rt64_shader_artifacts.py smoke-dxc-phases --port-dir /absolute/path/to/clean/rt64-port --oracle-dir /absolute/path/to/clean/rt64-oracle --dxc-dir /absolute/path/to/DirectXShaderCompiler --dxc-build-dir /outside/repo/dxc-build",
        "python3 tools/rt64_shader_artifacts.py build-validator --output-dir /outside/repo/wgpu-validator",
        "python3 tools/rt64_shader_artifacts.py verify-validator-build --build-dir /outside/repo/wgpu-validator",
        "python3 tools/rt64_shader_artifacts.py produce --port-dir /absolute/path/to/clean/rt64-port --oracle-dir /absolute/path/to/clean/rt64-oracle --dxc-dir /absolute/path/to/DirectXShaderCompiler --dxc-build-dir /outside/repo/dxc-build --wgpu-validator-build-dir /outside/repo/wgpu-validator --output-dir /outside/repo/rt64-artifacts",
        "```",
        "",
        "The ordinary fn64 build has no acquisition command. The future runtime consumer destination is owned by the `fn64-render-wgpu` integration ticket and must admit only a separately reviewed complete receipt/artifact set.",
        "",
        "Continuous improvement rule: after any producer change, run the qualified `smoke-dxc-phases` command before paying for validator or full-corpus execution. It exercises one real denominator entry and proves the selected DXC's dependency/preprocess semantics without making a corpus claim. This cheap gate exists because the first full attempt discovered that official DXC accepts `-P -MD -MF` while emitting no depfile.",
        "",
        "## Current blocker",
        "",
        "An official DXC source build and standalone validator build completed under the previous producer. The first corpus attempt failed closed on row one because official DXC v1.9.2607 accepts the combined `-P -MD -MF` invocation but emits the preprocessed file without a depfile. This repair separates dependency-only and preprocess-only phases. Because both build receipts bind the exact producer SHA, this source change invalidates those prior receipts by design: rebuild and reverify DXC and the validator, run `smoke-dxc-phases`, then execute all 56 rows. No corpus qualification claim exists until that sequence and independent receipt review pass.",
        "",
        "## Primary-source audit",
        "",
        "- Microsoft DirectXShaderCompiler `README.md`, `LICENSE.TXT`, `ThirdPartyNotices.txt`, `.gitmodules`, `docs/BuildingAndTestingDXC.rst`, `docs/SPIR-V.rst`, and `include/dxc/Support/HLSLOptions.td`, all at `0d3ee6b551b8fa768fbf825300ebab81047ef6a8`.",
        "- KhronosGroup SPIRV-Headers `LICENSE` and `LICENSES/*` at `29981f65241605e08b0ede4cfeb999fe3b723c6a`.",
        "- KhronosGroup SPIRV-Tools root and VS Code LSP `LICENSE` files at `b707790a898e44038547df54580022fc1cf89c3d`.",
        "- Microsoft DirectX-Headers `LICENSE` at `980971e835876dc0cde415e8f9bc646e64667bf7`.",
        "- MIT RT64 `CMakeLists.txt` and admitted shader/shared source at both frozen RT64 commits.",
        "",
    ]
    return "\n".join(lines)


def validate_dxc_source(tree: Path, require_complete: bool = False) -> dict:
    policy = load_policy()
    dxc = policy["dxc"]
    require(tree.is_dir(), "DXC source directory does not exist")
    require(git(tree, "rev-parse", "HEAD") == dxc["commit"], "DXC source is at the wrong commit")
    require(
        not git(tree, "status", "--porcelain", "--untracked-files=all", "--ignore-submodules=none"),
        "DXC source checkout or a submodule is dirty",
    )
    tag_kind = git(tree, "cat-file", "-t", dxc["tag"])
    tag_commit = git(tree, "rev-list", "-n", "1", dxc["tag"])
    require(tag_kind == "commit", "DXC tag unexpectedly changed from a lightweight commit tag")
    require(tag_commit == dxc["commit"], "DXC tag does not resolve to the pinned commit")
    if require_complete:
        validate_complete_pinned_git_worktree(
            tree,
            dxc["commit"],
            "DXC source build",
            policy["git_checkout_maximum_file_bytes"],
        )
    else:
        validate_pinned_git_worktree(
            tree,
            dxc["commit"],
            "DXC source checkout",
            allow_skip_worktree=True,
            maximum_file_bytes=policy["git_checkout_maximum_file_bytes"],
            verify_materialized_bytes=False,
        )
    reviewed_licenses = dxc["license_files"] + dxc["bundled_license_files"]
    for license_file in reviewed_licenses:
        actual = digest_bytes(git_blob(tree, dxc["commit"], license_file["path"]))
        require(actual == license_file["sha256"], f"DXC license byte drift: {license_file['path']}")
    tracked_licenses = set(
        git(tree, "ls-files", "*LICENSE*", "*COPYRIGHT*", "*Copyright*", "*COPYING*", "*NOTICE*", "*Notice*").splitlines()
    )
    expected_licenses = {
        item["path"] for item in reviewed_licenses if item["path"] != ".gitmodules"
    }
    require(tracked_licenses == expected_licenses, f"DXC tracked license closure changed: {sorted(tracked_licenses ^ expected_licenses)}")
    dependencies = []
    gitlinks: dict[str, str] = {}
    for line in git(tree, "ls-tree", "-r", "HEAD").splitlines():
        fields = line.split()
        if len(fields) >= 4 and fields[0] == "160000":
            gitlinks[fields[3]] = fields[2]
    expected_paths = {dependency["path"] for dependency in dxc["source_dependencies"]}
    require(set(gitlinks) == expected_paths, f"DXC gitlink closure changed: {sorted(gitlinks)}")
    for dependency in dxc["source_dependencies"]:
        path = tree / dependency["path"]
        require(path.is_dir(), f"DXC source dependency is not initialized: {dependency['path']}")
        require(gitlinks[dependency["path"]] == dependency["commit"], f"DXC gitlink drift: {dependency['path']}")
        require(git(path, "rev-parse", "HEAD") == dependency["commit"], f"DXC dependency checkout drift: {dependency['path']}")
        require(not git(path, "status", "--porcelain", "--untracked-files=all", "--ignore-submodules=none"), f"DXC dependency is dirty: {dependency['path']}")
        if require_complete:
            validate_complete_pinned_git_worktree(
                path,
                dependency["commit"],
                f"DXC source dependency {dependency['path']}",
                policy["git_checkout_maximum_file_bytes"],
            )
        else:
            validate_pinned_git_worktree(
                path,
                dependency["commit"],
                f"DXC dependency {dependency['path']}",
                allow_skip_worktree=True,
                maximum_file_bytes=policy["git_checkout_maximum_file_bytes"],
                verify_materialized_bytes=False,
            )
        nested_gitlinks = [
            line.split()[3]
            for line in git(path, "ls-tree", "-r", "HEAD").splitlines()
            if len(line.split()) >= 4 and line.split()[0] == "160000"
        ]
        require(not nested_gitlinks, f"DXC dependency has unreviewed nested gitlinks: {dependency['path']}")
        expected_dependency_licenses = {item["path"] for item in dependency["license_files"]}
        tracked_dependency_licenses = set(
            git(path, "ls-files", "*LICENSE*", "*COPYRIGHT*", "*Copyright*", "*COPYING*", "*NOTICE*", "*Notice*").splitlines()
        )
        require(
            tracked_dependency_licenses == expected_dependency_licenses,
            f"DXC dependency tracked license closure changed: {dependency['path']}",
        )
        for license_file in dependency["license_files"]:
            require(
                digest_file(path / license_file["path"]) == license_file["sha256"],
                f"DXC dependency license drift: {dependency['path']}/{license_file['path']}",
            )
        dependencies.append(copy.deepcopy(dependency))
    for excluded in dxc["excluded_declarations"]:
        require(excluded["path"] not in gitlinks, f"excluded DXC declaration became a gitlink: {excluded['path']}")
    return {
        "schema": "fn64.dxc-source-audit.v1",
        "repository": dxc["repository"],
        "tag": dxc["tag"],
        "tag_object_kind": tag_kind,
        "commit": dxc["commit"],
        "license_expression": dxc["license_expression"],
        "license_files": copy.deepcopy(dxc["license_files"]),
        "bundled_license_files": copy.deepcopy(dxc["bundled_license_files"]),
        "source_dependencies": dependencies,
        "excluded_declarations": copy.deepcopy(dxc["excluded_declarations"]),
    }


def executable(path_text: str) -> Path:
    path = Path(path_text)
    if not path.is_absolute():
        located = shutil.which(path_text)
        require(located is not None, f"tool is not on PATH: {path_text}")
        path = Path(located)
    path = path.absolute()
    info = path.stat()
    require(stat.S_ISREG(info.st_mode), f"tool is not a regular file: {path}")
    require(os.access(path, os.X_OK), f"tool is not executable: {path}")
    return path


def tool_record(path: Path, version_arguments: list[str], env: dict[str, str] | None = None) -> dict:
    result = subprocess.run([str(path), *version_arguments], capture_output=True, env=env)
    require(result.returncode == 0, f"tool version command failed: {path.name}")
    return {
        "name": path.name,
        "sha256": digest_file(path.resolve()),
        "version_arguments": version_arguments,
        "version_stdout_sha256": digest_bytes(result.stdout),
        "version_stderr_sha256": digest_bytes(result.stderr),
        "version_stdout": result.stdout.decode(errors="replace").strip(),
        "version_stderr": result.stderr.decode(errors="replace").strip(),
    }


def select_dxc_compiler(output: Path, candidates: list[Path], maximum: int) -> ContainedExecutable:
    present = [path for path in candidates if os.path.lexists(path)]
    require(present, "official DXC build completed without a dxc executable at a reviewed location")
    require(len(present) == 1, "official DXC build emitted multiple dxc invocation paths at reviewed locations")
    return qualify_contained_executable(output, present[0], maximum, "DXC compiler")


@dataclass(frozen=True)
class DxcCompilerClosure:
    root: Path
    compiler: ContainedExecutable
    runtime_files: tuple[tuple[ContainedExecutable, str], ...]
    inspector: Path
    receipt_record: dict


def inspect_otool_loads(inspector: Path, path: Path, label: str) -> tuple[list[dict], dict]:
    result = subprocess.run(
        [str(inspector), "-L", str(path)],
        capture_output=True,
        env={"PATH": "/usr/bin:/bin", "LC_ALL": "C", "LANG": "C"},
    )
    require(result.returncode == 0, f"runtime dependency inspection failed for {label}")
    try:
        lines = result.stdout.decode("utf-8").splitlines()
    except UnicodeDecodeError as error:
        raise ArtifactError(f"runtime dependency inspection was not UTF-8 for {label}") from error
    require(lines and lines[0].endswith(":"), f"runtime dependency inspection had no header for {label}")
    rows = []
    for line in lines[1:]:
        descriptor = line.strip()
        require(descriptor, f"runtime dependency inspection had an empty row for {label}")
        marker = " (compatibility version "
        require(marker in descriptor and descriptor.endswith(")"), f"runtime dependency row changed shape for {label}")
        load_name = descriptor.split(marker, 1)[0]
        require(load_name, f"runtime dependency row had no load name for {label}")
        rows.append({"load_name": load_name, "descriptor": descriptor})
    require(rows, f"runtime dependency inspection found no load commands for {label}")
    transcript = {
        "loads_sha256": digest_bytes(canonical_json(rows)),
        "stderr_sha256": digest_bytes(result.stderr),
    }
    return rows, transcript


def classify_macho_loads(rows: list[dict], retained: dict[str, dict], system_names: set[str], label: str) -> list[dict]:
    classified = []
    seen_retained: set[str] = set()
    seen_system: set[str] = set()
    for row in rows:
        name = row["load_name"]
        value = dict(row)
        if name in retained:
            require(name not in seen_retained, f"duplicate retained runtime dependency for {label}: {name}")
            seen_retained.add(name)
            value["classification"] = "retained-build-artifact"
            value["relative_path"] = retained[name]["relative_path"]
        elif name in system_names:
            require(name not in seen_system, f"duplicate system runtime dependency for {label}: {name}")
            seen_system.add(name)
            value["classification"] = "admitted-system-library"
        else:
            raise ArtifactError(f"unclassified non-system runtime dependency for {label}: {name}")
        classified.append(value)
    require(seen_retained == set(retained), f"retained runtime dependency denominator changed for {label}")
    require(seen_system == system_names, f"system runtime dependency denominator changed for {label}")
    return classified


def qualify_dxc_runtime_closure(output: Path, candidates: list[Path], policy: dict) -> DxcCompilerClosure:
    require(sys.platform == "darwin", "DXC runtime dependency qualification is currently implemented only for macOS")
    runtime_policy = policy["darwin_runtime_closure"]
    require(runtime_policy.get("format") == "otool-L-v1", "DXC runtime dependency inspection format changed")
    inspector = executable(runtime_policy["inspector"])
    compiler = select_dxc_compiler(output, candidates, policy["maximum_compiler_bytes"])
    retained_rows = runtime_policy.get("retained")
    require(isinstance(retained_rows, list) and retained_rows, "DXC retained runtime dependency policy is empty")
    retained = {row["load_name"]: row for row in retained_rows}
    require(len(retained) == len(retained_rows), "DXC retained runtime dependency policy repeats a load name")
    system_names = set(runtime_policy.get("system_load_names", []))
    require(system_names, "DXC system runtime dependency policy is empty")

    runtime_files = []
    qualified_retained = []
    seen_paths: set[str] = set()
    seen_snapshot_paths: set[str] = set()
    for row in retained_rows:
        relative = PurePosixPath(row.get("relative_path", ""))
        snapshot_relative = PurePosixPath(row.get("snapshot_relative_path", ""))
        require(
            relative.parts and not relative.is_absolute() and ".." not in relative.parts,
            "unsafe retained DXC runtime dependency path",
        )
        require(
            snapshot_relative.parts and not snapshot_relative.is_absolute() and ".." not in snapshot_relative.parts,
            "unsafe staged DXC runtime dependency path",
        )
        require(relative.as_posix() not in seen_paths, "retained DXC runtime dependency path is reused")
        require(snapshot_relative.as_posix() not in seen_snapshot_paths, "staged DXC runtime dependency path is reused")
        seen_paths.add(relative.as_posix())
        seen_snapshot_paths.add(snapshot_relative.as_posix())
        artifact = qualify_contained_executable(
            output,
            output.joinpath(*relative.parts),
            policy["maximum_runtime_dependency_bytes"],
            f"DXC runtime dependency {row['load_name']}",
        )
        require(artifact.receipt_record.get("kind") == "regular", f"DXC runtime dependency is not a regular retained file: {row['load_name']}")
        qualified_retained.append((row, artifact, snapshot_relative.as_posix()))
        runtime_files.append((artifact, snapshot_relative.as_posix()))

    provisional = DxcCompilerClosure(output, compiler, tuple(runtime_files), inspector, {})
    retained_records = []
    with staged_dxc_compiler(provisional, output) as staged_compiler:
        staged_root = staged_compiler.parent.parent
        compiler_loads, compiler_transcript = inspect_otool_loads(inspector, staged_compiler, "staged DXC compiler")
        classified_compiler_loads = classify_macho_loads(compiler_loads, retained, system_names, "DXC compiler")
        for row, artifact, snapshot_relative_text in qualified_retained:
            snapshot_relative = PurePosixPath(snapshot_relative_text)
            staged_dependency = staged_root.joinpath(*snapshot_relative.parts)
            dependency_loads, dependency_transcript = inspect_otool_loads(inspector, staged_dependency, row["load_name"])
            require(
                dependency_loads[0]["load_name"] == row.get("install_name"),
                f"DXC runtime dependency install name changed: {row['load_name']}",
            )
            classified_dependency_loads = classify_macho_loads(
                dependency_loads[1:],
                {},
                system_names,
                row["load_name"],
            )
            retained_records.append({
                "load_name": row["load_name"],
                "snapshot_relative_path": snapshot_relative.as_posix(),
                "install_name": dependency_loads[0],
                "artifact": artifact.receipt_record,
                "loads": classified_dependency_loads,
                "inspection": dependency_transcript,
            })
    record = {
        "platform": "darwin",
        "format": runtime_policy["format"],
        "compiler_artifact": compiler.receipt_record,
        "compiler_loads": classified_compiler_loads,
        "compiler_inspection": compiler_transcript,
        "retained": retained_records,
    }
    record["closure_sha256"] = digest_bytes(canonical_json(record))
    return DxcCompilerClosure(output, compiler, tuple(runtime_files), inspector, record)


def stage_qualified_file(source: Path, expected_sha256: str, destination: Path, maximum: int, mode: int, label: str) -> None:
    data, source_info = stable_regular_bytes(source, maximum, label)
    require(source_info.st_nlink == 1, f"{label} gained another hardlink")
    require(digest_bytes(data) == expected_sha256, f"{label} bytes differ from the qualified closure")
    write_new_private_file(destination, data)
    os.chmod(destination, mode)
    staged, staged_info = stable_regular_bytes(destination, maximum, f"staged {label}")
    require(staged_info.st_nlink == 1, f"staged {label} gained another hardlink")
    require(staged == data, f"staged {label} bytes differ from the qualified closure")


@contextmanager
def staged_dxc_compiler(closure: DxcCompilerClosure, parent: Path):
    with tempfile.TemporaryDirectory(prefix=".fn64-dxc-runtime-", dir=parent) as temporary:
        root = Path(temporary)
        root.chmod(0o700)
        compiler = root / "bin/dxc"
        stage_qualified_file(
            closure.compiler.target_path,
            closure.compiler.receipt_record["target_sha256"],
            compiler,
            load_policy()["dxc"]["maximum_compiler_bytes"],
            0o500,
            "DXC compiler",
        )
        for artifact, relative_text in closure.runtime_files:
            relative = PurePosixPath(relative_text)
            stage_qualified_file(
                artifact.target_path,
                artifact.receipt_record["target_sha256"],
                root.joinpath(*relative.parts),
                load_policy()["dxc"]["maximum_runtime_dependency_bytes"],
                0o400,
                f"DXC runtime dependency {relative.name}",
            )
        yield compiler


def dxc_closure_tool_record(closure: DxcCompilerClosure, version_arguments: list[str], parent: Path) -> dict:
    with staged_dxc_compiler(closure, parent) as compiler:
        result = subprocess.run(
            [str(compiler), *version_arguments],
            capture_output=True,
            env={"PATH": "/usr/bin:/bin", "LC_ALL": "C", "LANG": "C"},
        )
    require(result.returncode == 0, "staged DXC version command failed")
    return {
        "name": "dxc",
        "sha256": closure.compiler.receipt_record["target_sha256"],
        "version_arguments": version_arguments,
        "version_stdout_sha256": digest_bytes(result.stdout),
        "version_stderr_sha256": digest_bytes(result.stderr),
        "version_stdout": result.stdout.decode(errors="replace").strip(),
        "version_stderr": result.stderr.decode(errors="replace").strip(),
    }


def validate_tool_record(record: object, label: str) -> None:
    require_keys(
        record,
        {
            "name", "sha256", "version_arguments", "version_stdout_sha256",
            "version_stderr_sha256", "version_stdout", "version_stderr",
        },
        label,
    )
    require(isinstance(record["name"], str) and record["name"], f"{label} has no tool name")
    require(SHA256_RE.fullmatch(record["sha256"]) is not None, f"{label} has no tool digest")
    require(
        isinstance(record["version_arguments"], list)
        and all(isinstance(item, str) for item in record["version_arguments"]),
        f"{label} has malformed version arguments",
    )
    for key in ("version_stdout_sha256", "version_stderr_sha256"):
        require(SHA256_RE.fullmatch(record[key]) is not None, f"{label} has malformed {key}")
    require(isinstance(record["version_stdout"], str), f"{label} has malformed version stdout")
    require(isinstance(record["version_stderr"], str), f"{label} has malformed version stderr")


def validate_log_record(record: object, label: str) -> None:
    require_keys(record, {"exit_code", "stdout_sha256", "stderr_sha256"}, label)
    require(record["exit_code"] == 0, f"{label} did not succeed")
    require(SHA256_RE.fullmatch(record["stdout_sha256"]) is not None, f"{label} has malformed stdout digest")
    require(SHA256_RE.fullmatch(record["stderr_sha256"]) is not None, f"{label} has malformed stderr digest")


def add_receipt_hash(receipt: dict) -> dict:
    result = copy.deepcopy(receipt)
    result.pop("receipt_sha256", None)
    result["receipt_sha256"] = digest_bytes(canonical_json(result))
    return result


def validate_receipt_hash(receipt: dict) -> None:
    actual = receipt.get("receipt_sha256")
    require(isinstance(actual, str) and len(actual) == 64, "receipt lacks a SHA-256 identity")
    copy_without = copy.deepcopy(receipt)
    del copy_without["receipt_sha256"]
    require(actual == digest_bytes(canonical_json(copy_without)), "receipt identity mismatch")


def isolated_environment(cc: Path, cxx: Path, tool_paths: list[Path]) -> dict[str, str]:
    directories = []
    for path in [*tool_paths, cc, cxx]:
        directory = str(path.parent)
        if directory not in directories:
            directories.append(directory)
    for directory in ["/usr/bin", "/bin"]:
        if directory not in directories:
            directories.append(directory)
    env = {
        "PATH": os.pathsep.join(directories),
        "LC_ALL": "C",
        "LANG": "C",
        "CC": str(cc),
        "CXX": str(cxx),
        "GIT_CONFIG_GLOBAL": os.devnull,
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_NO_REPLACE_OBJECTS": "1",
        "GIT_OPTIONAL_LOCKS": "0",
        "GIT_TERMINAL_PROMPT": "0",
    }
    for name in ("SDKROOT", "MACOSX_DEPLOYMENT_TARGET", "SYSTEMROOT", "TEMP", "TMP", "TMPDIR"):
        if name in os.environ:
            env[name] = os.environ[name]
    return env


def run_logged(command: list[str], cwd: Path, env: dict[str, str]) -> dict:
    result = subprocess.run(command, cwd=cwd, env=env, capture_output=True)
    require(result.returncode == 0, f"command failed ({result.returncode}): {Path(command[0]).name}\n{result.stderr.decode(errors='replace')[-4000:]}")
    return {
        "exit_code": result.returncode,
        "stdout_sha256": digest_bytes(result.stdout),
        "stderr_sha256": digest_bytes(result.stderr),
    }


def relative_to(path: Path, parent: Path) -> Path | None:
    try:
        return path.relative_to(parent)
    except ValueError:
        return None


def dxc_source_component(path: str) -> str:
    if path.startswith("external/DirectX-Headers/"):
        return "directx-headers-mit"
    if path.startswith("external/SPIRV-Headers/"):
        return "spirv-headers-mit"
    if path.startswith("external/SPIRV-Tools/"):
        return "spirv-tools-apache-2.0"
    if path.startswith("lib/DxilCompression/"):
        return "dxc-bundled-dxil-compression-mit"
    if re.match(r"lib/Support/(?:reg[^/]*|COPYRIGHT\.regex)$", path):
        return "llvm-bundled-openbsd-regex"
    return "dxc-bundled-llvm-clang-ncsa-and-retained-notices"


def ninja_built_outputs(build: Path, ninja_log: Path) -> set[Path]:
    build = build.resolve()
    try:
        lines = ninja_log.read_text(encoding="utf-8").splitlines()
    except (OSError, UnicodeDecodeError) as error:
        raise ArtifactError(f"cannot read DXC .ninja_log: {error}") from error
    require(lines and lines[0].startswith("# ninja log v"), "DXC .ninja_log has no recognized header")
    outputs: set[Path] = set()
    for line in lines[1:]:
        if not line:
            continue
        fields = line.split("\t")
        require(len(fields) == 5, "malformed DXC .ninja_log row")
        path = Path(fields[3])
        if not path.is_absolute():
            path = build / path
        path = path.resolve()
        require(relative_to(path, build) is not None, "DXC .ninja_log output escaped the build root")
        outputs.add(path)
    require(outputs, "DXC .ninja_log has no executed outputs")
    return outputs


def compiled_source_manifest(source: Path, build: Path, compile_commands: Path, ninja_log: Path) -> dict:
    source = source.resolve()
    build = build.resolve()
    try:
        commands = json.loads(compile_commands.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ArtifactError(f"cannot read DXC compile_commands.json: {error}") from error
    require(isinstance(commands, list) and commands, "DXC compile_commands.json has no translation units")
    executed_outputs = ninja_built_outputs(build, ninja_log)
    command_outputs: set[Path] = set()
    selected_commands = []
    for command in commands:
        require(
            isinstance(command, dict)
            and isinstance(command.get("file"), str)
            and isinstance(command.get("directory"), str)
            and isinstance(command.get("output"), str),
            "DXC compile command lacks an exact file/directory/output identity",
        )
        directory = Path(command.get("directory", build))
        if not directory.is_absolute():
            directory = build / directory
        directory = directory.resolve()
        require(relative_to(directory, build) is not None, "DXC compile-command directory escaped the build root")
        output = Path(command["output"])
        if not output.is_absolute():
            output = directory / output
        output = output.resolve()
        require(relative_to(output, build) is not None, "DXC compile-command output escaped the build root")
        require(output not in command_outputs, "DXC compile_commands.json repeats an output")
        command_outputs.add(output)
        if output in executed_outputs:
            selected_commands.append(command)
    unmatched_objects = sorted(
        path.relative_to(build).as_posix()
        for path in executed_outputs - command_outputs
        if path.suffix.lower() in {".o", ".obj"}
    )
    require(not unmatched_objects, f"executed DXC object outputs lack compile-command identities: {unmatched_objects[:8]}")
    require(selected_commands, "DXC target executed no translation units recorded by compile_commands.json")

    rows: dict[str, dict] = {}
    for command in selected_commands:
        directory = Path(command["directory"])
        if not directory.is_absolute():
            directory = build / directory
        path = Path(command["file"])
        if not path.is_absolute():
            path = directory / path
        path = path.resolve()
        source_relative = relative_to(path, source)
        build_relative = relative_to(path, build)
        if source_relative is not None:
            relative = source_relative.as_posix()
            require(not relative.startswith(("test/", "tests/", "unittests/", "utils/unittest/")), f"test source entered the DXC compiler build: {relative}")
            row = {
                "path": f"source/{relative}",
                "sha256": digest_file(path),
                "component": dxc_source_component(relative),
            }
        elif build_relative is not None:
            relative = build_relative.as_posix()
            row = {
                "path": f"generated/{relative}",
                "sha256": digest_file(path),
                "component": "official-cmake-generated-source",
            }
        else:
            raise ArtifactError(f"DXC compile command names a translation unit outside source/build roots: {path.name}")
        prior = rows.get(row["path"])
        require(prior is None or prior == row, f"DXC translation unit identity differs across commands: {row['path']}")
        rows[row["path"]] = row
    ordered = [rows[path] for path in sorted(rows)]
    counts: dict[str, int] = {}
    for row in ordered:
        counts[row["component"]] = counts.get(row["component"], 0) + 1
    payload = {
        "schema": "fn64.dxc-compiled-source-manifest.v1",
        "selection": "fresh-ninja-target-executed-output-intersection",
        "translation_units": ordered,
        "counts_by_component": [
            {"component": component, "count": count} for component, count in sorted(counts.items())
        ],
    }
    payload["source_set_sha256"] = digest_bytes(canonical_json(ordered))
    require(not LOCAL_PATH_RE.search(json.dumps(payload)), "DXC compiled-source manifest leaked a machine-local path")
    return payload


def validate_compiled_source_files(manifest: dict, source: Path, build: Path) -> None:
    rows = manifest.get("translation_units")
    require(isinstance(rows, list) and rows, "DXC source manifest has no translation units")
    seen: set[str] = set()
    counts: dict[str, int] = {}
    for row in rows:
        require_keys(row, {"path", "sha256", "component"}, "DXC translation-unit row")
        relative = PurePosixPath(row.get("path", ""))
        require(relative.parts and not relative.is_absolute() and ".." not in relative.parts, "unsafe DXC translation-unit path")
        require(relative.as_posix() not in seen, f"duplicate DXC translation-unit path: {relative}")
        seen.add(relative.as_posix())
        require(SHA256_RE.fullmatch(row.get("sha256", "")) is not None, f"malformed DXC translation-unit digest: {relative}")
        kind = relative.parts[0]
        tail = Path(*relative.parts[1:])
        if kind == "source":
            path = source / tail
            require(row.get("component") == dxc_source_component(tail.as_posix()), f"DXC source component changed: {tail}")
        elif kind == "generated":
            path = build / tail
            require(row.get("component") == "official-cmake-generated-source", f"DXC generated-source component changed: {tail}")
        else:
            raise ArtifactError(f"unknown DXC translation-unit root: {kind}")
        counts[row["component"]] = counts.get(row["component"], 0) + 1
        require(digest_file(path) == row.get("sha256"), f"DXC translation-unit bytes changed: {tail}")
    expected_counts = [
        {"component": component, "count": count} for component, count in sorted(counts.items())
    ]
    require(manifest.get("counts_by_component") == expected_counts, "DXC source-manifest component counts changed")


def build_dxc(args: argparse.Namespace) -> dict:
    source = Path(args.dxc_dir).resolve()
    output = Path(args.output_dir).resolve()
    require(not output.exists(), "DXC output directory must not already exist")
    require(ROOT not in output.parents and output != ROOT, "DXC build output must stay outside the fn64 repository")
    source_audit = validate_dxc_source(source, require_complete=True)
    cmake = executable(args.cmake)
    ninja = executable(args.ninja)
    python = executable(args.python)
    git_tool = executable(args.git)
    cc = executable(args.cc)
    cxx = executable(args.cxx)
    output.mkdir(parents=True)
    build = output / "build"
    policy = load_policy()
    dxc_policy = policy["dxc"]
    env = isolated_environment(cc, cxx, [cmake, ninja, python, git_tool])
    configure = [
        str(cmake), "-S", str(source), "-B", str(build), "-G", "Ninja",
        "-C", str(source / dxc_policy["cmake_cache"]), *dxc_policy["cmake_flags"],
        f"-DPython3_EXECUTABLE={python}", f"-DGIT_EXECUTABLE={git_tool}",
    ]
    configure_log = run_logged(configure, output, env)
    build_command = [str(cmake), "--build", str(build), "--target", dxc_policy["build_target"], "--parallel", "1"]
    build_log = run_logged(build_command, output, env)
    command_graph = run_logged([str(ninja), "-C", str(build), "-t", "commands", dxc_policy["build_target"]], output, env)
    post_source_audit = validate_dxc_source(source, require_complete=True)
    require(post_source_audit == source_audit, "DXC source authority changed during configure/build")
    candidates = [build / "bin/dxc", build / "bin/dxc.exe", build / "Debug/bin/dxc.exe", build / "Release/bin/dxc.exe"]
    compiler = qualify_dxc_runtime_closure(output, candidates, dxc_policy)
    compile_commands = build / "compile_commands.json"
    cmake_cache = build / "CMakeCache.txt"
    build_ninja = build / "build.ninja"
    ninja_log = build / ".ninja_log"
    require(compile_commands.is_file(), "DXC build did not emit compile_commands.json")
    require(cmake_cache.is_file(), "DXC build did not emit CMakeCache.txt")
    require(build_ninja.is_file(), "DXC build did not emit build.ninja")
    require(ninja_log.is_file(), "DXC build did not emit .ninja_log execution evidence")
    manifest = compiled_source_manifest(source, build, compile_commands, ninja_log)
    manifest_path = output / "compiled-source-manifest.json"
    manifest_path.write_bytes(pretty_json(manifest))
    tools = {
        "cmake": tool_record(cmake, ["--version"]),
        "ninja": tool_record(ninja, ["--version"]),
        "python": tool_record(python, ["--version"]),
        "git": tool_record(git_tool, ["--version"]),
        "cc": tool_record(cc, ["--version"]),
        "cxx": tool_record(cxx, ["--version"]),
        "runtime_dependency_inspector": tool_record(
            compiler.inspector,
            ["--version"],
            {"PATH": "/usr/bin:/bin", "LC_ALL": "C", "LANG": "C"},
        ),
        "dxc": dxc_closure_tool_record(compiler, ["--version"], output),
    }
    receipt = add_receipt_hash({
        "schema": BUILD_SCHEMA,
        "status": "complete",
        "producer_sha256": digest_file(TOOL_PATH),
        "source": source_audit,
        "policy_sha256": digest_file(POLICY_PATH),
        "configuration": {
            "generator": "Ninja",
            "cache": dxc_policy["cmake_cache"],
            "flags": dxc_policy["cmake_flags"],
            "target": dxc_policy["build_target"],
            "parallel": 1,
            "dynamic_tool_bindings": ["GIT_EXECUTABLE", "Python3_EXECUTABLE"],
            "environment": [
                {"name": name, "value_sha256": digest_bytes(value.encode())}
                for name, value in sorted(env.items())
            ],
        },
        "tools": tools,
        "configure": configure_log,
        "build": build_log,
        "command_graph": command_graph,
        "cmake_cache_sha256": digest_file(cmake_cache),
        "build_ninja_sha256": digest_file(build_ninja),
        "ninja_log_sha256": digest_file(ninja_log),
        "compile_commands_sha256": digest_file(compile_commands),
        "compiled_source_manifest": {
            "path": "compiled-source-manifest.json",
            "sha256": digest_file(manifest_path),
            "source_set_sha256": manifest["source_set_sha256"],
            "translation_units": len(manifest["translation_units"]),
            "counts_by_component": manifest["counts_by_component"],
        },
        "compiler_closure": compiler.receipt_record,
        "compiler_sha256": compiler.compiler.receipt_record["target_sha256"],
        "claim_boundary": "local-source-build-integrity-not-transferable-process-attestation",
    })
    require(not LOCAL_PATH_RE.search(json.dumps(receipt)), "DXC build receipt leaked a machine-local path")
    (output / "dxc-build-receipt.json").write_bytes(pretty_json(receipt))
    return receipt


def validate_build_receipt(build_dir: Path, dxc_source: Path) -> tuple[dict, DxcCompilerClosure]:
    receipt = load_canonical_json(build_dir / "dxc-build-receipt.json", load_policy()["spirv"]["maximum_receipt_bytes"], "DXC build receipt")
    require_keys(receipt, {
        "schema", "status", "producer_sha256", "source", "policy_sha256", "configuration", "tools",
        "configure", "build", "command_graph", "cmake_cache_sha256", "build_ninja_sha256",
        "ninja_log_sha256", "compile_commands_sha256", "compiled_source_manifest", "compiler_closure",
        "compiler_sha256", "claim_boundary", "receipt_sha256",
    }, "DXC build receipt")
    require(receipt.get("schema") == BUILD_SCHEMA and receipt.get("status") == "complete", "DXC build receipt is incomplete")
    validate_receipt_hash(receipt)
    require(receipt.get("producer_sha256") == digest_file(TOOL_PATH), "DXC build used a different artifact producer")
    require(
        receipt.get("source") == validate_dxc_source(dxc_source, require_complete=True),
        "complete materialized DXC source audit does not match build receipt",
    )
    require(receipt.get("policy_sha256") == digest_file(POLICY_PATH), "DXC build used a different artifact policy")
    require(receipt.get("claim_boundary") == "local-source-build-integrity-not-transferable-process-attestation", "DXC claim boundary changed")
    require_keys(
        receipt.get("tools"),
        {"cmake", "ninja", "python", "git", "cc", "cxx", "runtime_dependency_inspector", "dxc"},
        "DXC tool closure",
    )
    for name, record in receipt["tools"].items():
        validate_tool_record(record, f"DXC {name} tool record")
    for name in ("configure", "build", "command_graph"):
        validate_log_record(receipt.get(name), f"DXC {name} transcript")
    policy = load_policy()["dxc"]
    require(
        receipt.get("configuration")
        and receipt["configuration"].get("generator") == "Ninja"
        and receipt["configuration"].get("cache") == policy["cmake_cache"]
        and receipt["configuration"].get("flags") == policy["cmake_flags"]
        and receipt["configuration"].get("target") == policy["build_target"]
        and receipt["configuration"].get("parallel") == 1
        and receipt["configuration"].get("dynamic_tool_bindings") == ["GIT_EXECUTABLE", "Python3_EXECUTABLE"],
        "DXC source-build configuration is not the reviewed configuration",
    )
    for path, key in (
        (build_dir / "build/CMakeCache.txt", "cmake_cache_sha256"),
        (build_dir / "build/build.ninja", "build_ninja_sha256"),
        (build_dir / "build/.ninja_log", "ninja_log_sha256"),
        (build_dir / "build/compile_commands.json", "compile_commands_sha256"),
    ):
        require(digest_file(path) == receipt.get(key), f"DXC retained build graph changed: {path.name}")
    manifest_record = receipt.get("compiled_source_manifest", {})
    require_keys(manifest_record, {"path", "sha256", "source_set_sha256", "translation_units", "counts_by_component"}, "DXC source-manifest record")
    manifest_relative = PurePosixPath(manifest_record.get("path", ""))
    require(manifest_relative == PurePosixPath("compiled-source-manifest.json"), "wrong DXC source-manifest path")
    manifest_path = build_dir / "compiled-source-manifest.json"
    require(digest_file(manifest_path) == manifest_record.get("sha256"), "DXC compiled-source manifest changed")
    manifest = load_canonical_json(
        manifest_path,
        policy["maximum_compiled_source_manifest_bytes"],
        "DXC source manifest",
    )
    require_keys(
        manifest,
        {"schema", "selection", "translation_units", "counts_by_component", "source_set_sha256"},
        "DXC source manifest",
    )
    require(manifest.get("schema") == "fn64.dxc-compiled-source-manifest.v1", "wrong DXC source-manifest schema")
    require(manifest.get("selection") == "fresh-ninja-target-executed-output-intersection", "wrong DXC source-manifest selection")
    require(manifest.get("source_set_sha256") == digest_bytes(canonical_json(manifest.get("translation_units"))), "DXC source-set identity mismatch")
    require(manifest_record.get("source_set_sha256") == manifest["source_set_sha256"], "DXC receipt source-set mismatch")
    require(manifest_record.get("translation_units") == len(manifest["translation_units"]), "DXC translation-unit count mismatch")
    require(manifest_record.get("counts_by_component") == manifest.get("counts_by_component"), "DXC component counts mismatch")
    reconstructed_manifest = compiled_source_manifest(
        dxc_source,
        build_dir / "build",
        build_dir / "build/compile_commands.json",
        build_dir / "build/.ninja_log",
    )
    require(manifest == reconstructed_manifest, "DXC compiled-source manifest does not match executed target outputs")
    validate_compiled_source_files(manifest, dxc_source, build_dir / "build")
    candidates = [
        build_dir / "build/bin/dxc",
        build_dir / "build/bin/dxc.exe",
        build_dir / "build/Debug/bin/dxc.exe",
        build_dir / "build/Release/bin/dxc.exe",
    ]
    compiler = qualify_dxc_runtime_closure(build_dir, candidates, policy)
    require(compiler.receipt_record == receipt.get("compiler_closure"), "DXC compiler runtime closure changed after the source build")
    require(
        compiler.compiler.receipt_record.get("target_sha256") == receipt.get("compiler_sha256"),
        "DXC compiler digest disagrees with its runtime closure",
    )
    require(
        receipt["tools"]["runtime_dependency_inspector"]
        == tool_record(
            compiler.inspector,
            ["--version"],
            {"PATH": "/usr/bin:/bin", "LC_ALL": "C", "LANG": "C"},
        ),
        "DXC runtime dependency inspector identity changed",
    )
    require(
        receipt["tools"]["dxc"]
        == dxc_closure_tool_record(compiler, ["--version"], build_dir),
        "DXC compiler tool identity changed",
    )
    return receipt, compiler


def validator_source_rows(policy: dict, root: Path | None = None) -> list[dict]:
    root = root or (ROOT / policy["source"])
    require(root.is_dir(), "validator source directory is missing")
    actual_files = {
        path.relative_to(root).as_posix()
        for path in root.rglob("*")
        if path.is_file() and "target" not in path.relative_to(root).parts
    }
    require(actual_files == set(policy["source_files"]), f"validator source file denominator changed: {sorted(actual_files ^ set(policy['source_files']))}")
    rows = []
    for relative in policy["source_files"]:
        path = root / relative
        rows.append({"path": relative, "sha256": digest_file(path)})
    require(digest_file(root / "Cargo.lock") == policy["cargo_lock_sha256"], "validator Cargo.lock drift")
    return rows


def stage_validator_source(source: Path, stage: Path, policy: dict) -> list[dict]:
    expected = validator_source_rows(policy, source)
    stage.mkdir(mode=0o700)
    for row in expected:
        data = stable_file_bytes(source / row["path"], 16 * 1024 * 1024, f"validator source {row['path']}")
        require(digest_bytes(data) == row["sha256"], f"validator source changed while staging: {row['path']}")
        write_new_private_file(stage / row["path"], data)
    require(validator_source_rows(policy, stage) == expected, "staged validator source identity mismatch")
    return expected


def cargo_configuration_files(working_directory: Path, cargo_home: Path) -> list[Path]:
    candidates = []
    for directory in (working_directory.resolve(), *working_directory.resolve().parents):
        candidates.extend((directory / ".cargo/config", directory / ".cargo/config.toml"))
    candidates.extend((cargo_home / "config", cargo_home / "config.toml"))
    return [path for path in candidates if path.exists()]


def require_isolated_cargo_configuration(working_directory: Path, cargo_home: Path) -> None:
    configs = cargo_configuration_files(working_directory, cargo_home)
    require(not configs, f"validator build has an ambient Cargo config: {configs[0].name if configs else ''}")


def direct_rust_toolchain(cargo: Path, rustc: Path) -> tuple[Path, Path]:
    sysroot_result = subprocess.run([str(rustc), "--print", "sysroot"], capture_output=True, text=True)
    require(sysroot_result.returncode == 0, "rustc did not report its toolchain sysroot")
    sysroot = Path(sysroot_result.stdout.strip()).resolve()
    suffix = ".exe" if os.name == "nt" else ""
    direct_cargo = sysroot / "bin" / f"cargo{suffix}"
    direct_rustc = sysroot / "bin" / f"rustc{suffix}"
    require(direct_cargo.is_file() and direct_rustc.is_file(), "Rust toolchain lacks direct cargo/rustc binaries")
    supplied = subprocess.run([str(cargo), "--version", "--verbose"], capture_output=True)
    direct = subprocess.run([str(direct_cargo), "--version", "--verbose"], capture_output=True)
    require(supplied.returncode == 0 and direct.returncode == 0, "cargo version query failed")
    require(supplied.stdout == direct.stdout and supplied.stderr == direct.stderr, "supplied cargo does not match rustc's direct toolchain")
    return direct_cargo, direct_rustc


def cargo_dependency_closure(
    cargo: Path,
    source: Path,
    working_directory: Path,
    env: dict[str, str],
    policy: dict,
) -> list[dict]:
    command = [
        str(cargo), "metadata", "--locked", "--format-version", "1",
        "--manifest-path", str(source / "Cargo.toml"),
    ]
    result = subprocess.run(command, cwd=working_directory, env=env, capture_output=True)
    detail = (result.stderr + result.stdout).decode(errors="replace")[-4000:]
    require(result.returncode == 0, f"validator cargo metadata failed: {detail}")
    try:
        metadata = json.loads(result.stdout)
    except json.JSONDecodeError as error:
        raise ArtifactError("validator cargo metadata was not JSON") from error
    packages = metadata.get("packages")
    require(isinstance(packages, list) and packages, "validator dependency closure is empty")
    allowed = set(policy["allowed_license_expressions"])
    rows = []
    for package in packages:
        name = package.get("name")
        version = package.get("version")
        license_expression = package.get("license")
        require(isinstance(name, str) and isinstance(version, str), "malformed validator dependency package")
        require(license_expression in allowed, f"validator dependency has unreviewed license: {name} {version} {license_expression!r}")
        source_identity = package.get("source")
        if name == policy["package"]:
            require(source_identity is None, "validator root package unexpectedly came from a registry")
            source_identity = "local-reviewed-probe"
        else:
            require(isinstance(source_identity, str) and source_identity.startswith("registry+"), f"validator dependency is not from the locked registry: {name}")
        rows.append({
            "name": name,
            "version": version,
            "license": license_expression,
            "source": source_identity,
        })
    rows.sort(key=lambda item: (item["name"], item["version"], item["source"]))
    require(sum(item["name"] == policy["package"] for item in rows) == 1, "validator root package is absent or duplicated")
    require(len(rows) == policy["dependency_package_count"], "validator dependency package count changed")
    require(digest_bytes(canonical_json(rows)) == policy["dependency_set_sha256"], "validator dependency closure changed")
    return rows


def validator_build_environment(
    cargo: Path,
    rustc: Path,
    cc: Path,
    cxx: Path,
    cargo_home: Path,
    target: Path,
) -> dict[str, str]:
    env = isolated_environment(cc, cxx, [cargo, rustc])
    env["CARGO_INCREMENTAL"] = "0"
    env["CARGO_HOME"] = str(cargo_home)
    env["CARGO_TARGET_DIR"] = str(target)
    env["RUSTC"] = str(rustc)
    env["CARGO_ENCODED_RUSTFLAGS"] = (
        f"--remap-path-prefix={cargo_home.parent}=/fn64/validator-build"
    )
    return env


def build_validator(args: argparse.Namespace) -> dict:
    policy = load_policy()["validator"]
    source = (ROOT / policy["source"]).resolve()
    output = Path(args.output_dir).resolve()
    prepare_output_directory(output)
    supplied_cargo = executable(args.cargo)
    supplied_rustc = executable(args.rustc)
    cargo, rustc = direct_rust_toolchain(supplied_cargo, supplied_rustc)
    cc = executable(args.cc)
    cxx = executable(args.cxx)
    staged_source = output / "source"
    cargo_home = output / "cargo-home"
    target = output / "target"
    source_rows = stage_validator_source(source, staged_source, policy)
    cargo_home.mkdir(mode=0o700)
    cargo_working_directory = Path(output.anchor)
    require_isolated_cargo_configuration(cargo_working_directory, cargo_home)
    env = validator_build_environment(cargo, rustc, cc, cxx, cargo_home, target)
    dependencies = cargo_dependency_closure(cargo, staged_source, cargo_working_directory, env, policy)
    command = [
        str(cargo), "build", "--locked", "--release",
        "--manifest-path", str(staged_source / "Cargo.toml"),
        "--target-dir", str(target),
        "--bin", policy["binary"],
    ]
    build_log = run_logged(command, cargo_working_directory, env)
    candidates = [target / "release" / policy["binary"], target / "release" / f"{policy['binary']}.exe"]
    binary = next((path for path in candidates if path.is_file()), None)
    require(binary is not None, "validator build completed without the reviewed binary")
    relative = binary.relative_to(output).as_posix()
    identity = validator_identity(binary)
    require(identity["identity"].get("wgpu_version") == policy["wgpu_version"], "validator wgpu version drift")
    require(identity["identity"].get("naga_version") == policy["naga_version"], "validator naga version drift")
    require(identity["identity"].get("backend") == policy["backend"], "validator backend drift")
    require(identity["identity"].get("validation") == policy["validation"], "validator validation mode drift")
    receipt = add_receipt_hash({
        "schema": VALIDATOR_BUILD_SCHEMA,
        "status": "complete",
        "producer_sha256": digest_file(TOOL_PATH),
        "policy_sha256": digest_file(POLICY_PATH),
        "source_files": source_rows,
        "source_set_sha256": digest_bytes(canonical_json(source_rows)),
        "cargo_lock_sha256": policy["cargo_lock_sha256"],
        "dependency_packages": dependencies,
        "dependency_set_sha256": digest_bytes(canonical_json(dependencies)),
        "configuration": {
            "locked": True,
            "profile": policy["build_profile"],
            "incremental": False,
            "binary": policy["binary"],
            "staged_source": "source",
            "cargo_home": "cargo-home",
            "target_dir": "target",
            "cargo_config": "none-in-filesystem-root-or-controlled-home",
            "cargo_working_directory": "filesystem-root",
            "rust_path_remap": "isolated-build-root=/fn64/validator-build",
            "rustc_explicit": True,
            "rustup_home_inherited": False,
        },
        "tools": {
            "cargo": tool_record(cargo, ["--version", "--verbose"]),
            "rustc": tool_record(rustc, ["--version", "--verbose"]),
            "cc": tool_record(cc, ["--version"]),
            "cxx": tool_record(cxx, ["--version"]),
        },
        "build": build_log,
        "binary_relative_path": relative,
        "binary_sha256": digest_file(binary),
        "validator_identity": identity,
        "claim_boundary": "local-source-build-integrity-not-transferable-process-attestation",
    })
    require(not LOCAL_PATH_RE.search(json.dumps(receipt)), "validator build receipt leaked a machine-local path")
    (output / "validator-build-receipt.json").write_bytes(pretty_json(receipt))
    return receipt


def validate_validator_build(build_dir: Path) -> tuple[dict, Path, dict]:
    policy = load_policy()["validator"]
    receipt = load_canonical_json(build_dir / "validator-build-receipt.json", load_policy()["spirv"]["maximum_receipt_bytes"], "validator build receipt")
    require_keys(receipt, {
        "schema", "status", "producer_sha256", "policy_sha256", "source_files", "source_set_sha256",
        "cargo_lock_sha256", "dependency_packages", "dependency_set_sha256", "configuration",
        "tools", "build", "binary_relative_path", "binary_sha256", "validator_identity",
        "claim_boundary", "receipt_sha256",
    }, "validator build receipt")
    require(receipt.get("schema") == VALIDATOR_BUILD_SCHEMA and receipt.get("status") == "complete", "validator build receipt is incomplete")
    validate_receipt_hash(receipt)
    require(receipt.get("producer_sha256") == digest_file(TOOL_PATH), "validator build used a different artifact producer")
    require(receipt.get("policy_sha256") == digest_file(POLICY_PATH), "validator build used a different policy")
    require(receipt.get("claim_boundary") == "local-source-build-integrity-not-transferable-process-attestation", "validator claim boundary changed")
    require_keys(receipt.get("tools"), {"cargo", "rustc", "cc", "cxx"}, "validator tool closure")
    for name, record in receipt["tools"].items():
        validate_tool_record(record, f"validator {name} tool record")
    validate_log_record(receipt.get("build"), "validator build transcript")
    source_rows = validator_source_rows(policy)
    require(receipt.get("source_files") == source_rows, "validator source files changed after build")
    staged_source = build_dir / "source"
    require(validator_source_rows(policy, staged_source) == source_rows, "retained staged validator source changed")
    require_isolated_cargo_configuration(Path(build_dir.anchor), build_dir / "cargo-home")
    require(receipt.get("source_set_sha256") == digest_bytes(canonical_json(source_rows)), "validator source-set identity mismatch")
    require(receipt.get("cargo_lock_sha256") == policy["cargo_lock_sha256"], "validator lock identity mismatch")
    dependencies = receipt.get("dependency_packages")
    require(isinstance(dependencies, list) and dependencies, "validator dependency closure is absent")
    require(receipt.get("dependency_set_sha256") == digest_bytes(canonical_json(dependencies)), "validator dependency-set identity mismatch")
    require(len(dependencies) == policy["dependency_package_count"], "validator dependency package count changed")
    require(receipt.get("dependency_set_sha256") == policy["dependency_set_sha256"], "validator dependency closure is not the reviewed closure")
    require(receipt.get("configuration") == {
        "locked": True,
        "profile": policy["build_profile"],
        "incremental": False,
        "binary": policy["binary"],
        "staged_source": "source",
        "cargo_home": "cargo-home",
        "target_dir": "target",
        "cargo_config": "none-in-filesystem-root-or-controlled-home",
        "cargo_working_directory": "filesystem-root",
        "rust_path_remap": "isolated-build-root=/fn64/validator-build",
        "rustc_explicit": True,
        "rustup_home_inherited": False,
    }, "validator build configuration changed")
    relative = PurePosixPath(receipt.get("binary_relative_path", ""))
    require(relative.parts and not relative.is_absolute() and ".." not in relative.parts, "unsafe validator binary path")
    binary = build_dir.joinpath(*relative.parts)
    require(digest_file(binary) == receipt.get("binary_sha256"), "validator binary changed after build")
    identity = validator_identity(binary)
    require(receipt.get("validator_identity") == identity, "validator protocol identity changed after build")
    return receipt, binary, identity


def write_denominator(port_dir: Path, oracle_dir: Path | None) -> dict:
    denominator = derive_denominator(port_dir, oracle_dir)
    DENOMINATOR_PATH.write_bytes(pretty_json(denominator))
    REPORT_PATH.write_text(render_report(denominator), encoding="utf-8")
    return denominator


def check_denominator(port_dir: Path, oracle_dir: Path | None) -> dict:
    expected = derive_denominator(port_dir, oracle_dir)
    actual = load_canonical_json(DENOMINATOR_PATH, 16 * 1024 * 1024, "RT64 shader denominator")
    require(actual == expected, "checked RT64 shader source denominator is stale")
    expected_report = render_report(expected)
    require(REPORT_PATH.read_text(encoding="utf-8") == expected_report, "generated RT64 shader report is stale")
    return expected


def prepare_output_directory(path: Path) -> None:
    require(not path.exists(), "artifact output directory must not already exist")
    require(ROOT not in path.parents and path != ROOT, "generated shader artifacts must stay outside the fn64 repository until reviewed")
    path.mkdir(parents=True)


def dependency_output_artifact(expected: dict) -> str:
    """Derive the retained raw depfile beside its normalized manifest."""
    manifest = PurePosixPath(expected["dependency_manifest_artifact"])
    require(manifest.suffix == ".json", f"dependency manifest path changed: {expected['id']}")
    return manifest.with_suffix(".d").as_posix()


def dxc_phase_contract(expected: dict) -> dict:
    """Return the path-stable contract independently checked by receipts."""
    forbidden = {"-M", "-MD", "-MF", "-P", "-Fi", "-Fo"}
    require(
        not forbidden.intersection(expected["flags"]),
        f"shader base flags contain a producer-owned phase option: {expected['id']}",
    )
    dependency_output = dependency_output_artifact(expected)
    return {
        "dependency": {
            "mode": "dxc-dependency-only-M",
            "source_input": expected["source"],
            "output": dependency_output,
            "phase_flags": ["-M", "-MF"],
        },
        "preprocess": {
            "mode": "dxc-preprocess-only-P",
            "source_input": expected["source"],
            "output": expected["preprocessed_artifact"],
            "phase_flags": ["-P", "-Fi"],
        },
        "compile": {
            "mode": "dxc-spirv-from-retained-preprocessed-input",
            "source_input": expected["preprocessed_artifact"],
            "output": expected["spirv_artifact"],
            "phase_flags": ["-Fo"],
        },
    }


def output_entry_set(root: Path) -> set[str]:
    return {
        path.relative_to(root).as_posix()
        for path in root.rglob("*")
        if path.is_file() or path.is_dir() or path.is_symlink()
    }


def require_exact_new_phase_output(
    root: Path,
    before: set[str],
    expected_relative: str,
    phase: str,
) -> None:
    after = output_entry_set(root)
    require(
        after - before == {expected_relative},
        f"DXC {phase} output set changed: {sorted(after - before)}",
    )
    require(
        before <= after,
        f"DXC {phase} removed a retained input: {sorted(before - after)}",
    )


def run_dxc(command: list[str], cwd: Path) -> dict:
    env = {"PATH": "/usr/bin:/bin", "LC_ALL": "C", "LANG": "C"}
    return run_logged(command, cwd, env)


def validator_identity(validator: Path) -> dict:
    result = subprocess.run([str(validator), "--fn64-version"], capture_output=True)
    require(result.returncode == 0, "wgpu validator version query failed")
    try:
        version = json.loads(result.stdout)
    except json.JSONDecodeError as error:
        raise ArtifactError("wgpu validator did not return its JSON identity") from error
    require_keys(version, {"schema", "wgpu_major", "wgpu_version", "naga_version", "backend", "validation"}, "wgpu validator identity")
    require(version.get("schema") == VALIDATOR_SCHEMA, "wrong wgpu validator protocol")
    require(version.get("wgpu_major") == 30, "shader validator is not wgpu 30")
    return {
        "binary_name": validator.name,
        "binary_sha256": digest_file(validator),
        "identity": version,
        "stdout_sha256": digest_bytes(result.stdout),
        "stderr_sha256": digest_bytes(result.stderr),
    }


def parse_dxc_dependency_rule(raw: bytes, expected: dict) -> list[str]:
    try:
        require(raw and b"\0" not in raw, f"DXC dependency output is empty or contains NUL: {expected['id']}")
        text = raw.decode("utf-8")
    except UnicodeDecodeError as error:
        raise ArtifactError(f"cannot read DXC dependency output: {error}") from error
    logical = text.replace("\\\r\n", " ").replace("\\\n", " ").strip()
    require(":" in logical, f"DXC dependency output has no target for {expected['id']}")
    require("\n" not in logical and "\r" not in logical, f"DXC dependency output has multiple or unterminated rules for {expected['id']}")
    target_text, dependency_text = logical.split(":", 1)
    try:
        targets = shlex.split(target_text)
        tokens = shlex.split(dependency_text)
    except ValueError as error:
        raise ArtifactError(f"malformed DXC dependency output for {expected['id']}: {error}") from error
    require(
        targets == [expected["source"]],
        f"DXC dependency target changed for {expected['id']}: {targets}",
    )
    require(tokens, f"DXC dependency output is empty for {expected['id']}")
    paths = []
    for token in tokens:
        path = PurePosixPath(token)
        require(
            not path.is_absolute() and ".." not in path.parts,
            f"DXC dependency escaped the private RT64 snapshot: {path.name}",
        )
        relative_text = posixpath.normpath(path.as_posix())
        require(relative_text == path.as_posix(), f"DXC dependency path is not canonical for {expected['id']}: {token}")
        require(relative_text in expected["dependency_files"], f"DXC observed an undeclared dependency for {expected['id']}: {relative_text}")
        if relative_text not in paths:
            paths.append(relative_text)
    paths.sort()
    require(expected["source"] in paths, f"DXC dependency output omitted the entry source: {expected['id']}")
    return paths


def parse_dxc_dependencies(depfile: Path, snapshot: Path, expected: dict, denominator: dict) -> dict:
    try:
        info = depfile.lstat()
        require(stat.S_ISREG(info.st_mode) and not depfile.is_symlink(), f"DXC dependency output is not a regular file: {expected['id']}")
        require(info.st_nlink == 1, f"DXC dependency output is reused through another hardlink: {expected['id']}")
        raw = stable_file_bytes(
            depfile,
            load_policy()["spirv"]["maximum_dependency_output_bytes"],
            f"DXC dependency output {expected['id']}",
        )
    except OSError as error:
        raise ArtifactError(f"cannot read DXC dependency output: {error}") from error
    paths = parse_dxc_dependency_rule(raw, expected)
    files = verify_staged_dependencies(
        snapshot,
        paths,
        denominator,
        load_policy()["spirv"]["maximum_source_bytes"],
    )
    return {
        "schema": DEPENDENCY_SCHEMA,
        "entry": expected["id"],
        "depfile_target": expected["source"],
        "files": files,
        "dependency_set_sha256": digest_bytes(canonical_json(files)),
    }


def prepare_dxc_shader_input(
    compiler: Path,
    snapshot: Path,
    output: Path,
    expected: dict,
    denominator: dict,
    policy: dict,
) -> dict:
    """Run dependency discovery and preprocessing as two disjoint DXC phases."""
    contract = dxc_phase_contract(expected)
    dependency_output = output / contract["dependency"]["output"]
    preprocessed = output / contract["preprocess"]["output"]
    dependency_output.parent.mkdir(parents=True, exist_ok=True)
    preprocessed.parent.mkdir(parents=True, exist_ok=True)
    require(not dependency_output.exists(), f"DXC dependency output path was reused: {expected['id']}")
    require(not preprocessed.exists(), f"preprocessed output path was reused: {expected['id']}")

    before_dependencies = verify_staged_dependencies(
        snapshot,
        expected["dependency_files"],
        denominator,
        policy["spirv"]["maximum_source_bytes"],
    )
    before_outputs = output_entry_set(output)
    dependency_command = [
        str(compiler),
        *expected["flags"],
        "-M",
        "-MF",
        str(dependency_output),
        expected["source"],
    ]
    dependency_log = run_dxc(dependency_command, snapshot)
    require_exact_new_phase_output(
        output,
        before_outputs,
        contract["dependency"]["output"],
        "dependency-only",
    )
    dependency_bytes = stable_file_bytes(
        dependency_output,
        policy["spirv"]["maximum_dependency_output_bytes"],
        f"DXC dependency output {expected['id']}",
    )
    active_dependencies = parse_dxc_dependencies(
        dependency_output,
        snapshot,
        expected,
        denominator,
    )
    after_dependency = verify_staged_dependencies(
        snapshot,
        expected["dependency_files"],
        denominator,
        policy["spirv"]["maximum_source_bytes"],
    )
    require(
        before_dependencies == after_dependency,
        f"staged dependency identity changed during dependency discovery: {expected['id']}",
    )
    os.chmod(dependency_output, 0o400)

    before_outputs = output_entry_set(output)
    preprocess_command = [
        str(compiler),
        *expected["flags"],
        "-P",
        "-Fi",
        str(preprocessed),
        expected["source"],
    ]
    preprocess_log = run_dxc(preprocess_command, snapshot)
    require_exact_new_phase_output(
        output,
        before_outputs,
        contract["preprocess"]["output"],
        "preprocess-only",
    )
    after_preprocess = verify_staged_dependencies(
        snapshot,
        expected["dependency_files"],
        denominator,
        policy["spirv"]["maximum_source_bytes"],
    )
    require(
        before_dependencies == after_preprocess,
        f"staged dependency identity changed during preprocessing: {expected['id']}",
    )
    require(
        dependency_bytes
        == stable_file_bytes(
            dependency_output,
            policy["spirv"]["maximum_dependency_output_bytes"],
            f"retained DXC dependency output {expected['id']}",
        ),
        f"DXC dependency output changed during preprocessing: {expected['id']}",
    )

    preprocessed_bytes = stable_file_bytes(
        preprocessed,
        policy["spirv"]["maximum_preprocessed_bytes"],
        f"preprocessed input {expected['id']}",
    )
    require(preprocessed.lstat().st_nlink == 1, f"preprocessed input has another link: {expected['id']}")
    try:
        preprocessed_text = preprocessed_bytes.decode("utf-8")
    except UnicodeDecodeError as error:
        raise ArtifactError(f"preprocessed input is not UTF-8: {expected['id']}") from error
    require(
        not INCLUDE_DIRECTIVE_RE.search(preprocessed_text),
        f"preprocessed input retains an include directive: {expected['id']}",
    )
    os.chmod(preprocessed, 0o400)
    require(
        (dependency_output.lstat().st_dev, dependency_output.lstat().st_ino)
        != (preprocessed.lstat().st_dev, preprocessed.lstat().st_ino),
        f"DXC phases reused one output file object: {expected['id']}",
    )
    return {
        "contract": contract,
        "dependency_path": dependency_output,
        "dependency_bytes": dependency_bytes,
        "dependency_log": dependency_log,
        "active_dependencies": active_dependencies,
        "preprocessed_path": preprocessed,
        "preprocessed_bytes": preprocessed_bytes,
        "preprocess_log": preprocess_log,
    }


def compile_dxc_shader(
    compiler: Path,
    snapshot: Path,
    output: Path,
    expected: dict,
    prepared: dict,
    policy: dict,
) -> dict:
    """Compile only the retained preprocessed input under the third phase."""
    contract = prepared["contract"]
    require(contract == dxc_phase_contract(expected), f"DXC prepared phase contract changed: {expected['id']}")
    output_root = output.resolve()
    require(output == output_root, f"artifact output root is not canonical: {expected['id']}")
    dependency_relative = PurePosixPath(contract["dependency"]["output"])
    preprocessed_relative = PurePosixPath(contract["compile"]["source_input"])
    for relative, label in (
        (dependency_relative, "dependency output"),
        (preprocessed_relative, "preprocessed input"),
    ):
        require(
            relative.parts and not relative.is_absolute() and ".." not in relative.parts,
            f"DXC {label} contract path is unsafe: {expected['id']}",
        )
    dependency_output = output_root.joinpath(*dependency_relative.parts)
    preprocessed = output_root.joinpath(*preprocessed_relative.parts)
    require(
        prepared["dependency_path"] == dependency_output,
        f"prepared dependency path does not match its phase contract: {expected['id']}",
    )
    require(
        prepared["preprocessed_path"] == preprocessed,
        f"prepared preprocessed path does not match its phase contract: {expected['id']}",
    )
    for path, label in (
        (dependency_output, "dependency output"),
        (preprocessed, "preprocessed input"),
    ):
        info = path.lstat()
        require(
            stat.S_ISREG(info.st_mode) and not path.is_symlink(),
            f"DXC {label} is not a regular no-link file: {expected['id']}",
        )
        require(info.st_nlink == 1, f"DXC {label} has another hardlink: {expected['id']}")
        require(path.resolve() == path, f"DXC {label} path is not canonical: {expected['id']}")
    dependency_bytes = prepared["dependency_bytes"]
    preprocessed_bytes = prepared["preprocessed_bytes"]
    artifact = output / contract["compile"]["output"]
    artifact.parent.mkdir(parents=True, exist_ok=True)
    require(not artifact.exists(), f"SPIR-V output path was reused: {expected['id']}")
    before_outputs = output_entry_set(output)
    compile_command = [
        str(compiler),
        *expected["flags"],
        str(preprocessed),
        "-Fo",
        str(artifact),
    ]
    compile_log = run_dxc(compile_command, snapshot)
    require_exact_new_phase_output(
        output,
        before_outputs,
        contract["compile"]["output"],
        "SPIR-V compile",
    )
    require(
        preprocessed_bytes
        == stable_file_bytes(
            preprocessed,
            policy["spirv"]["maximum_preprocessed_bytes"],
            f"compiled preprocessed input {expected['id']}",
        ),
        f"preprocessed input changed during compilation: {expected['id']}",
    )
    require(
        dependency_bytes
        == stable_file_bytes(
            dependency_output,
            policy["spirv"]["maximum_dependency_output_bytes"],
            f"compiled DXC dependency output {expected['id']}",
        ),
        f"DXC dependency output changed during compilation: {expected['id']}",
    )
    artifact_bytes = stable_file_bytes(
        artifact,
        policy["spirv"]["maximum_artifact_bytes"],
        f"SPIR-V artifact {expected['id']}",
    )
    require(artifact.lstat().st_nlink == 1, f"SPIR-V artifact has another link: {expected['id']}")
    artifact_info = artifact.lstat()
    earlier_objects = {
        (dependency_output.lstat().st_dev, dependency_output.lstat().st_ino),
        (preprocessed.lstat().st_dev, preprocessed.lstat().st_ino),
    }
    require(
        (artifact_info.st_dev, artifact_info.st_ino) not in earlier_objects,
        f"DXC compile reused an earlier phase output file object: {expected['id']}",
    )
    require(artifact_bytes.startswith(SPIRV_MAGIC), f"SPIR-V magic mismatch: {expected['id']}")
    return {
        "artifact_path": artifact,
        "artifact_bytes": artifact_bytes,
        "compile_log": compile_log,
    }


def run_wgpu_validation(validator: Path, artifact: Path, expected: dict) -> dict:
    validation = subprocess.run(
        [str(validator), "--shader", str(artifact), "--stage", expected["stage"], "--entry", expected["entry"]],
        capture_output=True,
        env={"PATH": "/usr/bin:/bin", "LC_ALL": "C", "LANG": "C"},
    )
    require(validation.returncode == 0, f"wgpu validation failed for {expected['id']}: {validation.stderr.decode(errors='replace')[-2000:]}")
    try:
        result = json.loads(validation.stdout)
    except json.JSONDecodeError as error:
        raise ArtifactError(f"wgpu validation result was not JSON for {expected['id']}") from error
    require(result == {
        "schema": "fn64.wgpu-shader-validation.v1",
        "status": "passed",
        "wgpu_major": 30,
        "stage": expected["stage"],
        "entry": expected["entry"],
        "module_bytes": artifact.stat().st_size,
    }, f"wgpu validation result changed for {expected['id']}")
    return {
        "status": "passed",
        "result": result,
        "stdout_sha256": digest_bytes(validation.stdout),
        "stderr_sha256": digest_bytes(validation.stderr),
    }


def smoke_dxc_phases(args: argparse.Namespace) -> dict:
    """Exercise the qualified compiler's dependency and preprocess semantics."""
    port = Path(args.port_dir).resolve()
    oracle = Path(args.oracle_dir).resolve() if args.oracle_dir else None
    denominator = check_denominator(port, oracle)
    dxc_source = Path(args.dxc_dir).resolve()
    build_receipt, compiler_closure = validate_build_receipt(
        Path(args.dxc_build_dir).resolve(),
        dxc_source,
    )
    policy = load_policy()
    expected = denominator["entries"][0]
    with tempfile.TemporaryDirectory(prefix="fn64-dxc-phase-smoke-") as temporary:
        root = Path(temporary)
        output = root / "artifacts"
        prepare_output_directory(output)
        snapshot = root / "source"
        snapshot_record = stage_rt64_source_snapshot(
            port,
            snapshot,
            denominator,
            policy["spirv"]["maximum_source_bytes"],
        )
        with staged_dxc_compiler(compiler_closure, root) as compiler:
            prepared = prepare_dxc_shader_input(
                compiler,
                snapshot,
                output,
                expected,
                denominator,
                policy,
            )
        result = {
            "schema": "fn64.dxc-phase-semantics-smoke.v1",
            "status": "passed",
            "producer_sha256": digest_file(TOOL_PATH),
            "dxc_build_receipt_sha256": build_receipt["receipt_sha256"],
            "dxc_compiler_sha256": build_receipt["compiler_sha256"],
            "source_snapshot_sha256": snapshot_record["source_set_sha256"],
            "entry": expected["id"],
            "source": expected["source"],
            "phase_contract": prepared["contract"],
            "dependency_output_sha256": digest_bytes(prepared["dependency_bytes"]),
            "dependency_output_bytes": len(prepared["dependency_bytes"]),
            "compiler_dependency_set_sha256": prepared["active_dependencies"]["dependency_set_sha256"],
            "compiler_dependency_count": len(prepared["active_dependencies"]["files"]),
            "preprocessed_sha256": digest_bytes(prepared["preprocessed_bytes"]),
            "preprocessed_bytes": len(prepared["preprocessed_bytes"]),
            "claim_boundary": "qualified-tool-semantics-smoke-not-corpus-qualification",
        }
    require(not LOCAL_PATH_RE.search(json.dumps(result)), "DXC phase smoke leaked a machine-local path")
    return result


def produce(args: argparse.Namespace) -> dict:
    port = Path(args.port_dir).resolve()
    oracle = Path(args.oracle_dir).resolve() if args.oracle_dir else None
    denominator = check_denominator(port, oracle)
    dxc_source = Path(args.dxc_dir).resolve()
    build_dir = Path(args.dxc_build_dir).resolve()
    build_receipt, compiler_closure = validate_build_receipt(build_dir, dxc_source)
    validator_build_receipt, validator, validator_record = validate_validator_build(Path(args.wgpu_validator_build_dir).resolve())
    output = Path(args.output_dir).resolve()
    prepare_output_directory(output)
    policy = load_policy()
    entries = []
    with (
        staged_dxc_compiler(compiler_closure, output) as compiler,
        tempfile.TemporaryDirectory(prefix=".fn64-rt64-source-", dir=output) as temporary,
    ):
        snapshot = Path(temporary) / "source"
        snapshot_record = stage_rt64_source_snapshot(
            port,
            snapshot,
            denominator,
            policy["spirv"]["maximum_source_bytes"],
        )
        require(
            snapshot_record["source_set_sha256"] == denominator["authority"]["port_source_set_sha256"],
            "private RT64 snapshot source-set identity mismatch",
        )
        source_sha_by_path = {row["path"]: row["sha256"] for row in snapshot_record["files"]}
        for expected in denominator["entries"]:
            dependency_manifest = output / expected["dependency_manifest_artifact"]
            dependency_manifest.parent.mkdir(parents=True, exist_ok=True)
            prepared = prepare_dxc_shader_input(
                compiler,
                snapshot,
                output,
                expected,
                denominator,
                policy,
            )
            contract = prepared["contract"]
            dependency_bytes = prepared["dependency_bytes"]
            active_dependencies = prepared["active_dependencies"]
            preprocessed_bytes = prepared["preprocessed_bytes"]
            compiled = compile_dxc_shader(
                compiler,
                snapshot,
                output,
                expected,
                prepared,
                policy,
            )
            artifact = compiled["artifact_path"]
            artifact_bytes = compiled["artifact_bytes"]
            dependency_manifest.write_bytes(pretty_json(active_dependencies))
            validation = run_wgpu_validation(validator, artifact, expected)
            entries.append({
                **expected,
                "source_sha256": source_sha_by_path[expected["source"]],
                "preprocessed_sha256": digest_bytes(preprocessed_bytes),
                "preprocessed_bytes": len(preprocessed_bytes),
                "dependency_output_artifact": contract["dependency"]["output"],
                "dependency_output_sha256": digest_bytes(dependency_bytes),
                "dependency_output_bytes": len(dependency_bytes),
                "dependency_manifest_sha256": digest_file(dependency_manifest, policy["spirv"]["maximum_dependency_manifest_bytes"]),
                "dependency_manifest_bytes": dependency_manifest.stat().st_size,
                "compiler_dependency_target": active_dependencies["depfile_target"],
                "compiler_dependency_files": active_dependencies["files"],
                "compiler_dependency_set_sha256": active_dependencies["dependency_set_sha256"],
                "spirv_sha256": digest_bytes(artifact_bytes),
                "spirv_bytes": len(artifact_bytes),
                "compiler": {
                    "base_flags": expected["flags"],
                    "phase_contract": contract,
                    "dependency_stdout_sha256": prepared["dependency_log"]["stdout_sha256"],
                    "dependency_stderr_sha256": prepared["dependency_log"]["stderr_sha256"],
                    "preprocess_stdout_sha256": prepared["preprocess_log"]["stdout_sha256"],
                    "preprocess_stderr_sha256": prepared["preprocess_log"]["stderr_sha256"],
                    "compile_stdout_sha256": compiled["compile_log"]["stdout_sha256"],
                    "compile_stderr_sha256": compiled["compile_log"]["stderr_sha256"],
                    "built_in_spirv_validation": "passed",
                },
                "wgpu_validation": validation,
            })
    artifact_set = [{"path": row["spirv_artifact"], "sha256": row["spirv_sha256"]} for row in entries]
    receipt = add_receipt_hash({
        "schema": RECEIPT_SCHEMA,
        "status": "complete",
        "producer_sha256": digest_file(TOOL_PATH),
        "policy_sha256": digest_file(POLICY_PATH),
        "denominator_sha256": denominator["denominator_sha256"],
        "source_snapshot": snapshot_record,
        "dxc_build_receipt_sha256": build_receipt["receipt_sha256"],
        "dxc_compiler_sha256": build_receipt["compiler_sha256"],
        "validator_build_receipt_sha256": validator_build_receipt["receipt_sha256"],
        "wgpu_validator": validator_record,
        "required_validation": policy["spirv"]["required_validation"],
        "entries": entries,
        "artifact_set_sha256": digest_bytes(canonical_json(artifact_set)),
        "claim_boundary": "complete-local-artifact-integrity-not-transferable-process-attestation",
    })
    require(not LOCAL_PATH_RE.search(json.dumps(receipt)), "artifact receipt leaked a machine-local path")
    (output / "receipt.json").write_bytes(pretty_json(receipt))
    return receipt


def validate_artifact_receipt(
    receipt: dict,
    denominator: dict,
    artifact_dir: Path,
    build_receipt: dict,
    validator_build_receipt: dict,
    validator: Path,
    validator_record: dict,
) -> None:
    policy = load_policy()
    require_keys(receipt, {
        "schema", "status", "producer_sha256", "policy_sha256", "denominator_sha256", "source_snapshot",
        "dxc_build_receipt_sha256", "dxc_compiler_sha256",
        "validator_build_receipt_sha256", "wgpu_validator", "required_validation",
        "entries", "artifact_set_sha256", "claim_boundary", "receipt_sha256",
    }, "shader artifact receipt")
    require(receipt.get("schema") == RECEIPT_SCHEMA and receipt.get("status") == "complete", "shader artifact receipt is incomplete")
    validate_receipt_hash(receipt)
    require(receipt.get("producer_sha256") == digest_file(TOOL_PATH), "artifacts used a different producer")
    require(receipt.get("policy_sha256") == digest_file(POLICY_PATH), "artifact policy identity mismatch")
    require(receipt.get("denominator_sha256") == denominator["denominator_sha256"], "artifact denominator identity mismatch")
    require(receipt.get("source_snapshot") == source_snapshot_record(denominator), "private RT64 source snapshot identity mismatch")
    require(receipt.get("dxc_build_receipt_sha256") == build_receipt["receipt_sha256"], "DXC build receipt identity mismatch")
    require(receipt.get("dxc_compiler_sha256") == build_receipt["compiler_sha256"], "DXC compiler identity mismatch")
    require(receipt.get("validator_build_receipt_sha256") == validator_build_receipt["receipt_sha256"], "validator build receipt identity mismatch")
    require(receipt.get("wgpu_validator") == validator_record, "wgpu validator identity mismatch")
    require(receipt.get("required_validation") == policy["spirv"]["required_validation"], "validation denominator changed")
    require(receipt.get("claim_boundary") == "complete-local-artifact-integrity-not-transferable-process-attestation", "artifact claim boundary changed")
    rows = receipt.get("entries")
    require(isinstance(rows, list) and len(rows) == len(denominator["entries"]), "artifact entry denominator changed")
    receipt_path = artifact_dir / "receipt.json"
    receipt_info = receipt_path.lstat()
    require(receipt_info.st_nlink == 1, "hardlinked artifact receipt is not admitted")
    require(digest_file(receipt_path, policy["spirv"]["maximum_receipt_bytes"]) == digest_bytes(pretty_json(receipt)), "artifact receipt bytes are not canonical")
    seen_files: set[tuple[int, int]] = {(receipt_info.st_dev, receipt_info.st_ino)}
    expected_files = {"receipt.json"}
    artifact_set = []
    source_by_path = {row["path"]: row for row in denominator["source_files"]}
    for expected, row in zip(denominator["entries"], rows, strict=True):
        require_keys(row, set(expected) | {
            "source_sha256", "preprocessed_sha256", "preprocessed_bytes",
            "dependency_output_artifact", "dependency_output_sha256", "dependency_output_bytes",
            "dependency_manifest_sha256", "dependency_manifest_bytes",
            "compiler_dependency_target", "compiler_dependency_files", "compiler_dependency_set_sha256",
            "spirv_sha256", "spirv_bytes", "compiler", "wgpu_validation",
        }, f"artifact row {expected['id']}")
        require_keys(row.get("compiler"), {
            "base_flags", "phase_contract",
            "dependency_stdout_sha256", "dependency_stderr_sha256",
            "preprocess_stdout_sha256", "preprocess_stderr_sha256",
            "compile_stdout_sha256", "compile_stderr_sha256", "built_in_spirv_validation",
        }, f"compiler receipt {expected['id']}")
        require_keys(row.get("wgpu_validation"), {
            "status", "result", "stdout_sha256", "stderr_sha256",
        }, f"wgpu receipt {expected['id']}")
        for transcript_name in (
            "dependency_stdout_sha256",
            "dependency_stderr_sha256",
            "preprocess_stdout_sha256",
            "preprocess_stderr_sha256",
            "compile_stdout_sha256",
            "compile_stderr_sha256",
        ):
            transcript_digest = row["compiler"][transcript_name]
            require(
                isinstance(transcript_digest, str) and SHA256_RE.fullmatch(transcript_digest),
                f"compiler transcript digest is not canonical SHA-256: {expected['id']} -> {transcript_name}",
            )
        for key, value in expected.items():
            require(row.get(key) == value, f"artifact row {expected['id']} changed field {key}")
        phase_contract = dxc_phase_contract(expected)
        require(row.get("compiler", {}).get("base_flags") == expected["flags"], f"artifact flags changed: {expected['id']}")
        require(row.get("compiler", {}).get("phase_contract") == phase_contract, f"DXC phase contract changed: {expected['id']}")
        require(
            phase_contract["compile"]["source_input"] == expected["preprocessed_artifact"],
            f"artifact was not compiled from retained preprocessed input: {expected['id']}",
        )
        require(
            row.get("dependency_output_artifact") == phase_contract["dependency"]["output"],
            f"raw dependency output path changed: {expected['id']}",
        )
        require(
            row.get("compiler_dependency_target") == expected["source"],
            f"compiler dependency target changed: {expected['id']}",
        )
        require(row.get("compiler", {}).get("built_in_spirv_validation") == "passed", f"DXC validation absent: {expected['id']}")
        require(row.get("wgpu_validation", {}).get("status") == "passed", f"wgpu validation absent: {expected['id']}")
        require(row.get("source_sha256") == source_by_path[expected["source"]]["port_sha256"], f"entry source identity mismatch: {expected['id']}")
        compiler_dependencies = row.get("compiler_dependency_files")
        require(isinstance(compiler_dependencies, list) and compiler_dependencies, f"compiler dependency closure absent: {expected['id']}")
        observed_paths = []
        for index, dependency in enumerate(compiler_dependencies):
            require_keys(
                dependency,
                {"path", "sha256"},
                f"compiler dependency {expected['id']}[{index}]",
            )
            path = dependency["path"]
            sha256 = dependency["sha256"]
            require(isinstance(path, str), f"compiler dependency path is not text: {expected['id']}[{index}]")
            require(
                isinstance(sha256, str) and SHA256_RE.fullmatch(sha256),
                f"compiler dependency digest is not canonical SHA-256: {expected['id']} -> {path}",
            )
            require(path in source_by_path, f"unknown compiler dependency: {expected['id']}")
            require(sha256 == source_by_path[path]["port_sha256"], f"compiler dependency digest changed: {expected['id']} -> {path}")
            observed_paths.append(path)
        require(observed_paths == sorted(set(observed_paths)), f"compiler dependency paths are not canonical: {expected['id']}")
        require(
            row.get("compiler_dependency_set_sha256") == digest_bytes(canonical_json(compiler_dependencies)),
            f"compiler dependency-set identity mismatch: {expected['id']}",
        )
        require(set(observed_paths) <= set(expected["dependency_files"]), f"compiler observed an undeclared dependency: {expected['id']}")
        require(expected["source"] in observed_paths, f"compiler dependency closure omitted source: {expected['id']}")
        for path_key, digest_key, size_key, maximum in (
            ("preprocessed_artifact", "preprocessed_sha256", "preprocessed_bytes", policy["spirv"]["maximum_preprocessed_bytes"]),
            ("dependency_output_artifact", "dependency_output_sha256", "dependency_output_bytes", policy["spirv"]["maximum_dependency_output_bytes"]),
            ("dependency_manifest_artifact", "dependency_manifest_sha256", "dependency_manifest_bytes", policy["spirv"]["maximum_dependency_manifest_bytes"]),
            ("spirv_artifact", "spirv_sha256", "spirv_bytes", policy["spirv"]["maximum_artifact_bytes"]),
        ):
            relative = PurePosixPath(row[path_key])
            expected_files.add(relative.as_posix())
            require(not relative.is_absolute() and ".." not in relative.parts, f"unsafe artifact path: {relative}")
            path = artifact_dir.joinpath(*relative.parts)
            info = path.lstat()
            identity = (info.st_dev, info.st_ino)
            require(identity not in seen_files, f"artifact file object reused through multiple paths: {relative}")
            seen_files.add(identity)
            require(info.st_nlink == 1, f"hardlinked artifact is not admitted: {relative}")
            require(info.st_size == row.get(size_key), f"artifact size mismatch: {relative}")
            require(digest_file(path, maximum) == row.get(digest_key), f"artifact digest mismatch: {relative}")
        dependency_manifest = load_canonical_json(
            artifact_dir / expected["dependency_manifest_artifact"],
            policy["spirv"]["maximum_dependency_manifest_bytes"],
            f"compiler dependency manifest {expected['id']}",
        )
        dependency_output = stable_file_bytes(
            artifact_dir / row["dependency_output_artifact"],
            policy["spirv"]["maximum_dependency_output_bytes"],
            f"retained DXC dependency output {expected['id']}",
        )
        require(
            parse_dxc_dependency_rule(dependency_output, expected)
            == [item["path"] for item in compiler_dependencies],
            f"raw and normalized compiler dependencies disagree: {expected['id']}",
        )
        require(dependency_manifest == {
            "schema": DEPENDENCY_SCHEMA,
            "entry": expected["id"],
            "depfile_target": expected["source"],
            "files": compiler_dependencies,
            "dependency_set_sha256": row["compiler_dependency_set_sha256"],
        }, f"compiler dependency manifest content mismatch: {expected['id']}")
        spirv_path = artifact_dir / expected["spirv_artifact"]
        require(spirv_path.read_bytes().startswith(SPIRV_MAGIC), f"SPIR-V magic mismatch: {expected['id']}")
        require(row.get("wgpu_validation") == run_wgpu_validation(validator, spirv_path, expected), f"wgpu validation transcript changed: {expected['id']}")
        artifact_set.append({"path": expected["spirv_artifact"], "sha256": row["spirv_sha256"]})
    require(receipt.get("artifact_set_sha256") == digest_bytes(canonical_json(artifact_set)), "artifact set identity mismatch")
    actual_files = {
        path.relative_to(artifact_dir).as_posix()
        for path in artifact_dir.rglob("*")
        if path.is_file() or path.is_symlink()
    }
    require(actual_files == expected_files, f"artifact file denominator changed: {sorted(actual_files ^ expected_files)}")
    require(not LOCAL_PATH_RE.search(json.dumps(receipt)), "artifact receipt contains a machine-local path")


def verify(args: argparse.Namespace) -> None:
    port = Path(args.port_dir).resolve()
    oracle = Path(args.oracle_dir).resolve() if args.oracle_dir else None
    denominator = check_denominator(port, oracle)
    dxc_source = Path(args.dxc_dir).resolve()
    build_dir = Path(args.dxc_build_dir).resolve()
    build_receipt, _ = validate_build_receipt(build_dir, dxc_source)
    validator_build_receipt, validator, validator_record = validate_validator_build(Path(args.wgpu_validator_build_dir).resolve())
    artifact_dir = Path(args.artifact_dir).resolve()
    receipt = load_canonical_json(
        artifact_dir / "receipt.json",
        load_policy()["spirv"]["maximum_receipt_bytes"],
        "shader artifact receipt",
    )
    validate_artifact_receipt(
        receipt,
        denominator,
        artifact_dir,
        build_receipt,
        validator_build_receipt,
        validator,
        validator_record,
    )


def selftest() -> None:
    policy = load_policy()
    require(policy["dxc"]["commit"] == "0d3ee6b551b8fa768fbf825300ebab81047ef6a8", "selftest pin drift")
    base = add_receipt_hash({"schema": "test", "value": 1})
    validate_receipt_hash(base)
    bad = copy.deepcopy(base)
    bad["value"] = 2
    try:
        validate_receipt_hash(bad)
    except ArtifactError:
        pass
    else:
        raise ArtifactError("receipt hash mutation was accepted")
    sample = b'build_pixel_shader(rt64 "src/shaders/X.hlsl")\n'
    calls = parse_cmake_shader_calls(sample)
    require(len(calls) == 1 and calls[0]["function"] == "build_pixel_shader", "CMake parser selftest failed")
    try:
        parse_cmake_shader_calls(b'build_unknown_shader(rt64 "src/shaders/X.hlsl")\n')
    except ArtifactError:
        pass
    else:
        raise ArtifactError("unknown CMake shader producer was accepted")


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    sub = result.add_subparsers(dest="command", required=True)
    for name in ("generate", "check"):
        item = sub.add_parser(name)
        item.add_argument("--port-dir", required=True)
        item.add_argument("--oracle-dir")
    audit = sub.add_parser("audit-dxc")
    audit.add_argument("--dxc-dir", required=True)
    build = sub.add_parser("build-dxc")
    build.add_argument("--dxc-dir", required=True)
    build.add_argument("--output-dir", required=True)
    build.add_argument("--cmake", default="cmake")
    build.add_argument("--ninja", default="ninja")
    build.add_argument("--python", default="python3")
    build.add_argument("--git", default="git")
    build.add_argument("--cc", default="cc")
    build.add_argument("--cxx", default="c++")
    validator = sub.add_parser("build-validator")
    validator.add_argument("--output-dir", required=True)
    validator.add_argument("--cargo", default="cargo")
    validator.add_argument("--rustc", default="rustc")
    validator.add_argument("--cc", default="cc")
    validator.add_argument("--cxx", default="c++")
    dxc_verify = sub.add_parser("verify-dxc-build")
    dxc_verify.add_argument("--dxc-dir", required=True)
    dxc_verify.add_argument("--build-dir", required=True)
    validator_verify = sub.add_parser("verify-validator-build")
    validator_verify.add_argument("--build-dir", required=True)
    phase_smoke = sub.add_parser("smoke-dxc-phases")
    phase_smoke.add_argument("--port-dir", required=True)
    phase_smoke.add_argument("--oracle-dir")
    phase_smoke.add_argument("--dxc-dir", required=True)
    phase_smoke.add_argument("--dxc-build-dir", required=True)
    for name in ("produce", "verify"):
        item = sub.add_parser(name)
        item.add_argument("--port-dir", required=True)
        item.add_argument("--oracle-dir")
        item.add_argument("--dxc-dir", required=True)
        item.add_argument("--dxc-build-dir", required=True)
        item.add_argument("--wgpu-validator-build-dir", required=True)
        if name == "produce":
            item.add_argument("--output-dir", required=True)
        else:
            item.add_argument("--artifact-dir", required=True)
    sub.add_parser("selftest")
    return result


def main() -> int:
    args = parser().parse_args()
    try:
        if args.command == "generate":
            value = write_denominator(Path(args.port_dir).resolve(), Path(args.oracle_dir).resolve() if args.oracle_dir else None)
            print(f"wrote {DENOMINATOR_PATH.relative_to(ROOT)} and {REPORT_PATH.relative_to(ROOT)}: {value['denominator_sha256']}")
        elif args.command == "check":
            value = check_denominator(Path(args.port_dir).resolve(), Path(args.oracle_dir).resolve() if args.oracle_dir else None)
            print(f"RT64 shader source denominator clean: {value['denominator_sha256']}")
        elif args.command == "audit-dxc":
            value = validate_dxc_source(Path(args.dxc_dir).resolve())
            print(json.dumps(value, indent=2))
        elif args.command == "build-dxc":
            value = build_dxc(args)
            print(f"DXC source build receipt: {value['receipt_sha256']}")
        elif args.command == "build-validator":
            value = build_validator(args)
            print(f"wgpu validator source build receipt: {value['receipt_sha256']}")
        elif args.command == "verify-dxc-build":
            value, _ = validate_build_receipt(Path(args.build_dir).resolve(), Path(args.dxc_dir).resolve())
            print(f"DXC source build verified: {value['receipt_sha256']}")
        elif args.command == "verify-validator-build":
            value, _, _ = validate_validator_build(Path(args.build_dir).resolve())
            print(f"wgpu validator source build verified: {value['receipt_sha256']}")
        elif args.command == "smoke-dxc-phases":
            value = smoke_dxc_phases(args)
            print(json.dumps(value, indent=2, sort_keys=True))
        elif args.command == "produce":
            value = produce(args)
            print(f"RT64 shader artifact receipt: {value['receipt_sha256']}")
        elif args.command == "verify":
            verify(args)
            print("RT64 shader artifact receipt and files verified")
        else:
            selftest()
            print("RT64 shader artifact selftest passed")
    except (ArtifactError, OSError, ValueError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
