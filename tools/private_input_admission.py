#!/usr/bin/env python3
"""Admit private fn64 inputs without copying content or identities into git."""

from __future__ import annotations

import argparse
import errno
import hashlib
import json
import os
import re
import secrets
import shutil
import stat
import subprocess
import sys
import tempfile
from pathlib import Path


MANIFEST_SCHEMA = "fn64.private-input-admission.v7"
LEGACY_MANIFEST_SCHEMA = "fn64.private-input-admission.v6"
READINESS_SCHEMA = "fn64.private-input-readiness.v6"
LEGACY_READINESS_SCHEMA = "fn64.private-input-readiness.v5"
PRIVATE_RUN_CONTRACT_SCHEMA = "fn64.private-release-run-contract.v3"
PRIVATE_RUN_CONTRACT_DIGEST_DOMAIN = (
    b"fn64.private-release-run-contract-digest.v3\0"
)
PROGRAM_BUILD_RECEIPT_SCHEMA = "fn64.release-program-build-receipt.v1"
PROGRAM_BUILD_RECEIPT_DIGEST_DOMAIN = (
    b"fn64.release-program-build-receipt-digest.v1\0"
)
PURPOSES = {
    "extended_gbi", "f3dzex2_characterization", "full_rom", "combined",
}
LEGACY_PURPOSES = {"extended_gbi", "full_rom", "combined"}
LEGACY_WIRE_FAMILIES = {
    "f3dex2_extended_gbi_v1",
    "f3dex2",
    "fast3d_f3dex",
    "s2dex_s2dex2",
    "full_rom_mixed",
}
WIRE_FAMILIES = LEGACY_WIRE_FAMILIES | {"f3dzex2"}
EXTENDED_CASES = {
    "hook-control",
    "disabled-negative-control",
    "activation",
    "widescreen",
    "interpolation",
    "vertex-z",
}
PLATFORMS = {"macos_arm64", "linux_x86_64", "windows_x86_64"}
CONTROLLERS = {
    "standard_controller", "controller_pak", "rumble_pak", "transfer_pak",
    "voice_recognition_unit",
}
SAVES = {
    "no_cartridge_save", "eeprom_4_kbit", "eeprom_16_kbit",
    "sram_32_kib", "flash_ram_128_kib",
}
RENDERERS = {
    "reference_lle_accuracy", "rt64_lle_accuracy", "rt64_post_vi_capture",
    "rt64_replacement_packs",
}
ROM_CLASSES = {"retail_cartridge", "public_homebrew"}
ROM_PROVENANCE_BY_CLASS = {
    "retail_cartridge": "user_owned_retail_cartridge_dump",
    "public_homebrew": "publicly_distributed_homebrew_rom",
}
ROLE_PROVENANCE = {
    "microcode_text": {"user_owned_rom_derived"},
    "microcode_data": {"user_owned_rom_derived"},
    "microcode_text_raw_window": {"user_owned_rom_derived"},
    "microcode_data_raw_window": {"user_owned_rom_derived"},
    "rom": set(ROM_PROVENANCE_BY_CLASS.values()),
    "recompiled": {"user_generated_from_owned_rom"},
}
ROOT_FIELDS = {
    "schema", "purpose", "intent", "release_matrix", "artifacts", "runner",
}
PROGRAM_EVIDENCE_LANES = {
    "no_program_fixture", "identified_native_archive",
    "typed_observed_function", "typed_block_program",
}
INTENT_FIELDS = {
    "wire_family", "report_scenario", "recognition", "extended_gbi_cases",
    "program_evidence_lane", "rom_class",
}
RELEASE_FIELDS = {"platform", "controllers", "save", "renderers", "repeat_bar"}
LEGACY_ARTIFACT_FIELDS = {
    "microcode_text", "microcode_data", "rom", "recompiled",
}
ARTIFACT_FIELDS = LEGACY_ARTIFACT_FIELDS | {
    "microcode_text_raw_window", "microcode_data_raw_window",
}
FILE_FIELDS = {"path", "length", "sha256", "provenance", "git_identity"}
RUNNER_FIELDS = {
    "executable", "working_directory", "argv", "env", "release_gate_cycle",
    "execution_source", "program_build_receipt",
}
EXECUTABLE_FIELDS = {"path", "length", "sha256", "git_identity"}
CONTRACT_DESCRIPTOR_FIELDS = {"path", "bytes", "sha256"}
CONTRACT_INPUT_DESCRIPTOR_FIELDS = {
    "role", "path", "bytes", "sha256", "provenance",
}
CONTRACT_FIELDS = {
    "schema", "admission_manifest", "readiness_report", "purpose",
    "program_build_receipt", "rom_class", "report_scenario", "guest_cycle", "repeat_count", "input",
    "admitted_artifacts", "expected_execution_source", "child",
    "contract_sha256",
}
CONTRACT_CHILD_FIELDS = {
    "executable", "working_directory", "argv", "environment",
}
CONTRACT_ENVIRONMENT_FIELDS = {"name", "value"}
EXECUTION_SOURCE_FIELDS = {
    "no_program": {"kind"},
    "native_archive": {"kind", "artifact_sha256"},
    "typed_observed_function_program": {"kind", "artifact_sha256"},
    "typed_block_program": {
        "kind", "program_sha256", "dispatch_artifact_sha256",
    },
}
LANE_EXECUTION_SOURCE_KIND = {
    "no_program_fixture": "no_program",
    "identified_native_archive": "native_archive",
    "typed_observed_function": "typed_observed_function_program",
    "typed_block_program": "typed_block_program",
}
# These values are generated per child by the trusted runner. A manifest may
# configure the fixed gate cycle and other game inputs, but it may not select
# output identity, run ordinal, or run-event identity.
RESERVED_RUNNER_ENV = {
    "ROM",
    "FN64_RELEASE_GATE_CYCLE",
    "FN64_RELEASE_REPORT",
    "FN64_RELEASE_RUN_EVENT_SHA256",
    "FN64_PRIVATE_RUN_CONTRACT",
    "FN64_PRIVATE_RUN_CONTRACT_SHA256",
    "FN64_PRIVATE_RUN_ORDINAL",
    "FN64_PRIVATE_RUN_ID",
    "FN64_RELEASE_ROM_CLASS",
}
FORBIDDEN_RUNNER_ENV_PREFIXES = (
    "LD_",
    "DYLD_",
    "PYTHON",
    "PERL",
    "RUBY",
    "NODE_",
    "LUA_",
    "TCL_",
    "DOTNET_",
    "MONO_",
    "POWERSHELL_",
    "GTK_",
    "QT_",
    "VK_",
    "LIBGL_",
    "MESA_",
    "__GL_",
    "D3D12SDK",
    "DXVK_",
    "VKD3D_",
)
FORBIDDEN_RUNNER_ENV = {
    "PATH",
    "PATHEXT",
    "COMSPEC",
    "BASH_ENV",
    "ENV",
    "SHELLOPTS",
    "ZDOTDIR",
    "GCONV_PATH",
    "LOCPATH",
    "NLSPATH",
    "CLASSPATH",
    "JAVA_TOOL_OPTIONS",
    "JDK_JAVA_OPTIONS",
    "_JAVA_OPTIONS",
    "SSLKEYLOGFILE",
    "NODE_OPTIONS",
    "GBM_BACKEND",
    "GALLIUM_DRIVER",
    "EGL_PLATFORM",
}
ENV_NAME_RE = re.compile(r"[A-Z_][A-Z0-9_]*\Z")
READINESS_FIELDS = {
    "schema", "status", "purpose", "wire_family", "report_scenario",
    "artifact_roles_admitted", "extended_gbi_fixture", "full_rom_inputs",
    "release_matrix_policy", "repeat_bar", "required_extended_cases",
    "platform", "controllers", "save", "renderers", "program_evidence_lane",
    "program_build_receipt", "rom_class",
}
PROGRAM_BUILD_RECEIPT_FIELDS = {
    "schema", "child_executable", "lane", "expected_execution_source",
    "receipt_sha256",
}
PROGRAM_FILE_IDENTITY_FIELDS = {"path", "bytes", "sha256"}
NATIVE_ARCHIVE_INPUT_FIELDS = {"label", "file"}
PROGRAM_BUILD_LANE_FIELDS = {
    "native_archives": {"kind", "archives"},
    "typed_observed_function": {"kind", "identity_wire"},
    "typed_block": {"kind", "pack", "expected_program_sha256"},
}
MAX_ARTIFACT_BYTES = 8 * 1024 * 1024 * 1024
SCENARIO_RE = re.compile(r"[a-z0-9][a-z0-9._-]{0,127}\Z")


class AdmissionError(Exception):
    pass


def require(condition: bool, message: str) -> None:
    if not condition:
        raise AdmissionError(message)


def reject_duplicate_object_pairs(pairs: list[tuple[str, object]]) -> dict:
    value: dict = {}
    for key, item in pairs:
        require(key not in value, f"JSON object contains duplicate field {key!r}")
        value[key] = item
    return value


def load_json(path: Path) -> dict:
    try:
        descriptor = open_regular_nofollow(path)
        try:
            mode = os.fstat(descriptor).st_mode
            require(stat.S_ISREG(mode), f"{path}: input must be a regular file")
            with os.fdopen(descriptor, "r", encoding="utf-8", closefd=False) as file:
                value = json.load(
                    file,
                    object_pairs_hook=reject_duplicate_object_pairs,
                )
        finally:
            os.close(descriptor)
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise AdmissionError(f"cannot read {path}: {error}") from error
    require(isinstance(value, dict), f"{path}: root must be an object")
    return value


def nonempty(value: object, where: str) -> str:
    require(isinstance(value, str) and bool(value.strip()), f"{where} must be nonempty")
    return value


def unique_strings(value: object, where: str) -> list[str]:
    require(isinstance(value, list), f"{where} must be an array")
    require(all(isinstance(item, str) and item for item in value), f"{where} entries must be nonempty strings")
    require(len(value) == len(set(value)), f"{where} contains duplicates")
    return value


def validate_sha256(value: object, where: str) -> str:
    require(isinstance(value, str) and len(value) == 64, f"{where} must be a SHA-256")
    require(all(character in "0123456789abcdef" for character in value), f"{where} must be lowercase hexadecimal")
    return value


def path_components(path: Path) -> list[Path]:
    components: list[Path] = []
    current = Path(path.anchor)
    for part in path.parts[1:]:
        current = current / part
        components.append(current)
    return components


def reject_symlink_components(path: Path, include_leaf: bool) -> None:
    components = path_components(path)
    if not include_leaf and components:
        components.pop()
    for component in components:
        if component.exists() or component.is_symlink():
            require(not component.is_symlink(), f"symlink path component is forbidden: {component}")


def same_file_identity(left: os.stat_result, right: os.stat_result) -> bool:
    return left.st_dev == right.st_dev and left.st_ino == right.st_ino


def stable_file_identity(value: os.stat_result) -> tuple[int, int, int, int, int]:
    return (
        value.st_dev,
        value.st_ino,
        value.st_size,
        value.st_mtime_ns,
        value.st_ctime_ns,
    )


def secure_dir_fd_available() -> bool:
    return (
        os.open in os.supports_dir_fd
        and hasattr(os, "O_DIRECTORY")
        and hasattr(os, "O_NOFOLLOW")
    )


def secure_publish_dir_fd_available() -> bool:
    return (
        secure_dir_fd_available()
        and os.link in os.supports_dir_fd
        and os.unlink in os.supports_dir_fd
    )


def open_directory_nofollow(path: Path) -> int:
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0)
    directory_flag = getattr(os, "O_DIRECTORY", 0)
    nofollow_flag = getattr(os, "O_NOFOLLOW", 0)
    if secure_dir_fd_available():
        descriptor = os.open(path.anchor, flags | directory_flag)
        try:
            for component in path.parts[1:]:
                next_descriptor = os.open(
                    component,
                    flags | directory_flag | nofollow_flag,
                    dir_fd=descriptor,
                )
                os.close(descriptor)
                descriptor = next_descriptor
            return descriptor
        except BaseException:
            os.close(descriptor)
            raise
    return os.open(path, flags | directory_flag | nofollow_flag)


def open_regular_nofollow(path: Path, flags: int = os.O_RDONLY, mode: int = 0o600) -> int:
    # O_NONBLOCK prevents a hostile FIFO substituted for an expected file from
    # hanging before fstat can reject it; it has no effect on regular files.
    open_flags = (
        flags
        | getattr(os, "O_CLOEXEC", 0)
        | getattr(os, "O_NOFOLLOW", 0)
        | getattr(os, "O_NONBLOCK", 0)
    )
    if secure_dir_fd_available():
        parent_descriptor = open_directory_nofollow(path.parent)
        try:
            return os.open(path.name, open_flags, mode, dir_fd=parent_descriptor)
        finally:
            os.close(parent_descriptor)
    return os.open(path, open_flags, mode)


def regular_file_measurement(path: Path) -> tuple[int, str, int]:
    try:
        descriptor = open_regular_nofollow(path)
    except OSError as error:
        raise AdmissionError(f"cannot open regular file {path}: {error}") from error
    digest = hashlib.sha256()
    try:
        before = os.fstat(descriptor)
        require(stat.S_ISREG(before.st_mode), f"{path} must be a regular file")
        with os.fdopen(descriptor, "rb", closefd=False) as file:
            while block := file.read(1024 * 1024):
                digest.update(block)
        after = os.fstat(descriptor)
        require(
            stable_file_identity(before) == stable_file_identity(after),
            f"{path} changed while it was being measured",
        )
        return before.st_size, digest.hexdigest(), before.st_mode
    finally:
        os.close(descriptor)


def inspect_regular_file(path: Path) -> os.stat_result:
    try:
        descriptor = open_regular_nofollow(path)
    except OSError as error:
        raise AdmissionError(f"cannot open regular file {path}: {error}") from error
    try:
        retained = os.fstat(descriptor)
        require(stat.S_ISREG(retained.st_mode), f"{path} must be a regular file")
        return retained
    finally:
        os.close(descriptor)


def stored_entry_name(parent: Path, child: Path, child_stat: os.stat_result) -> str:
    matches: list[str] = []
    try:
        with os.scandir(parent) as entries:
            for entry in entries:
                try:
                    entry_stat = entry.stat(follow_symlinks=False)
                except OSError:
                    continue
                if same_file_identity(entry_stat, child_stat):
                    matches.append(entry.name)
    except OSError as error:
        raise AdmissionError(f"cannot inspect repository path component {parent}: {error}") from error
    require(matches, f"repository path changed while inspecting {child}")
    if child.name in matches:
        return child.name
    case_matches = [name for name in matches if name.casefold() == child.name.casefold()]
    if len(case_matches) == 1:
        return case_matches[0]
    require(len(matches) == 1, f"ambiguous hard-linked repository path component {child}")
    return matches[0]


def filesystem_relative_to(path: Path, root: Path) -> Path | None:
    """Return the on-disk spelling below root, using identities for containment."""
    try:
        root_stat = os.stat(root, follow_symlinks=False)
    except OSError as error:
        raise AdmissionError(f"cannot inspect repository root {root}: {error}") from error

    current = path
    missing: list[str] = []
    while True:
        try:
            current_stat = os.stat(current, follow_symlinks=False)
            break
        except FileNotFoundError:
            require(current != current.parent, f"cannot locate an existing ancestor of {path}")
            missing.append(current.name)
            current = current.parent
        except OSError as error:
            raise AdmissionError(f"cannot inspect path {current}: {error}") from error

    existing_components: list[str] = []
    while not same_file_identity(current_stat, root_stat):
        parent = current.parent
        if parent == current:
            return None
        try:
            parent_stat = os.stat(parent, follow_symlinks=False)
        except OSError as error:
            raise AdmissionError(f"cannot inspect path ancestor {parent}: {error}") from error
        existing_components.append(stored_entry_name(parent, current, current_stat))
        current = parent
        current_stat = parent_stat

    return Path(*reversed(existing_components), *reversed(missing))


def run_git_path_check(arguments: list[str], root: Path, operation: str) -> int:
    result = subprocess.run(
        ["git", *arguments],
        cwd=root,
        capture_output=True,
    )
    require(
        result.returncode in {0, 1},
        f"git {operation} failed with status {result.returncode}",
    )
    return result.returncode


def git_excluded(path: Path, root: Path) -> bool:
    relative = filesystem_relative_to(path, root)
    if relative is None:
        return True
    tracked = run_git_path_check(
        ["ls-files", "--error-unmatch", "--", str(relative)],
        root,
        "ls-files",
    ) == 0
    if tracked:
        return False
    return run_git_path_check(
        ["check-ignore", "-q", "--no-index", "--", str(relative)],
        root,
        "check-ignore",
    ) == 0


def validate_local_regular_file(path_text: object, where: str, root: Path) -> Path:
    path = Path(nonempty(path_text, f"{where}.path"))
    require(path.is_absolute(), f"{where}.path must be absolute")
    require(".." not in path.parts, f"{where}.path must not contain '..'")
    reject_symlink_components(path, include_leaf=True)
    inspect_regular_file(path)
    require(git_excluded(path, root), f"{where}.path is inside the repository and not gitignored")
    return path


def validate_local_directory(path_text: object, where: str, root: Path) -> Path:
    path = Path(nonempty(path_text, where))
    require(path.is_absolute(), f"{where} must be absolute")
    require(".." not in path.parts, f"{where} must not contain '..'")
    reject_symlink_components(path, include_leaf=True)
    try:
        if secure_dir_fd_available():
            descriptor = open_directory_nofollow(path)
            try:
                mode = os.fstat(descriptor).st_mode
            finally:
                os.close(descriptor)
        else:
            mode = os.lstat(path).st_mode
        require(stat.S_ISDIR(mode), f"{where} must be a directory")
    except OSError as error:
        raise AdmissionError(f"{where} cannot be inspected: {error}") from error
    require(
        git_excluded(path, root),
        f"{where} is inside the repository and not gitignored",
    )
    return path


def sha256_file(path: Path) -> str:
    return regular_file_measurement(path)[1]


def validate_artifact(role: str, descriptor: object, root: Path) -> Path:
    require(isinstance(descriptor, dict) and set(descriptor) == FILE_FIELDS, f"artifacts.{role}: invalid fields")
    require(descriptor["git_identity"] == "excluded", f"artifacts.{role}.git_identity must be 'excluded'")
    require(descriptor["provenance"] in ROLE_PROVENANCE[role], f"artifacts.{role}.provenance is invalid")
    length = descriptor["length"]
    require(isinstance(length, int) and not isinstance(length, bool) and 0 < length <= MAX_ARTIFACT_BYTES, f"artifacts.{role}.length is invalid")
    if role == "microcode_text":
        require(length == 4096, "artifacts.microcode_text.length must be the exact 4 KiB IMEM image")
    elif role == "microcode_text_raw_window":
        require(
            length == 0x18D0,
            "artifacts.microcode_text_raw_window.length must be the exact 0x18d0-byte native recognition window",
        )
    elif role == "microcode_data_raw_window":
        require(
            length == 0x0FC0,
            "artifacts.microcode_data_raw_window.length must be the exact 0x0fc0-byte native recognition window",
        )
    expected_hash = validate_sha256(descriptor["sha256"], f"artifacts.{role}.sha256")
    path = validate_local_regular_file(descriptor["path"], f"artifacts.{role}", root)
    actual_length, actual_hash, _ = regular_file_measurement(path)
    require(actual_length == length, f"artifacts.{role} length drift: expected {length}, observed {actual_length}")
    require(actual_hash == expected_hash, f"artifacts.{role} SHA-256 drift")
    return path


def validate_contract_descriptor(
    value: object, where: str, root: Path,
) -> tuple[dict, Path]:
    require(
        isinstance(value, dict) and set(value) == CONTRACT_DESCRIPTOR_FIELDS,
        f"{where}: invalid descriptor fields",
    )
    length = value["bytes"]
    require(
        isinstance(length, int) and not isinstance(length, bool)
        and 0 < length <= MAX_ARTIFACT_BYTES,
        f"{where}.bytes is invalid",
    )
    expected_hash = validate_sha256(value["sha256"], f"{where}.sha256")
    path = validate_local_regular_file(value["path"], where, root)
    actual_length, actual_hash, _ = regular_file_measurement(path)
    require(actual_length == length, f"{where} length drift")
    require(actual_hash == expected_hash, f"{where} SHA-256 drift")
    return value, path


def validate_contract_artifact_descriptor(
    value: object, where: str, root: Path,
) -> tuple[dict, Path]:
    require(
        isinstance(value, dict)
        and set(value) == CONTRACT_INPUT_DESCRIPTOR_FIELDS,
        f"{where}: invalid artifact descriptor fields",
    )
    role = nonempty(value["role"], f"{where}.role")
    require(role in ARTIFACT_FIELDS, f"{where}.role is invalid")
    require(
        value["provenance"] in ROLE_PROVENANCE[role],
        f"{where}.provenance is invalid",
    )
    descriptor = {
        "path": value["path"],
        "bytes": value["bytes"],
        "sha256": value["sha256"],
    }
    _, path = validate_contract_descriptor(descriptor, where, root)
    return value, path


def validate_execution_source(value: object, lane: str, where: str) -> dict:
    require(isinstance(value, dict), f"{where} must be an object")
    kind = nonempty(value.get("kind"), f"{where}.kind")
    expected_kind = LANE_EXECUTION_SOURCE_KIND[lane]
    require(
        kind == expected_kind,
        f"{where}.kind {kind!r} does not match program lane {lane!r}",
    )
    require(
        set(value) == EXECUTION_SOURCE_FIELDS[kind],
        f"{where} fields are invalid for {kind!r}",
    )
    for field in sorted(set(value) - {"kind"}):
        validate_sha256(value[field], f"{where}.{field}")
    return value


def require_native_executable(path: Path, where: str) -> None:
    """Require the same native-image envelope as the Rust process runner."""
    descriptor = open_regular_nofollow(path)
    try:
        magic = os.read(descriptor, 4)
        elf = magic == b"\x7fELF"
        mach_o = magic in {
            b"\xfe\xed\xfa\xce", b"\xce\xfa\xed\xfe",
            b"\xfe\xed\xfa\xcf", b"\xcf\xfa\xed\xfe",
            b"\xca\xfe\xba\xbe", b"\xbe\xba\xfe\xca",
            b"\xca\xfe\xba\xbf", b"\xbf\xba\xfe\xca",
        }
        portable_executable = False
        if magic[:2] == b"MZ":
            try:
                os.lseek(descriptor, 0x3C, os.SEEK_SET)
                offset_bytes = os.read(descriptor, 4)
                if len(offset_bytes) == 4:
                    offset = int.from_bytes(offset_bytes, "little")
                    os.lseek(descriptor, offset, os.SEEK_SET)
                    portable_executable = os.read(descriptor, 4) == b"PE\0\0"
            except OSError:
                portable_executable = False
    finally:
        os.close(descriptor)
    require(
        elf or mach_o or portable_executable,
        f"{where} must be a native ELF, Mach-O, or PE image; scripts are forbidden",
    )
    if os.name == "posix":
        require(
            os.access(path, os.X_OK),
            f"{where} has native image bytes but no Unix execute bit",
        )


def validate_runner(
    runner: object,
    lane: str,
    root: Path,
    recompiled_descriptor: dict | None = None,
) -> tuple[dict, Path]:
    require(
        isinstance(runner, dict) and set(runner) == RUNNER_FIELDS,
        "runner fields are invalid",
    )
    executable = runner["executable"]
    require(
        isinstance(executable, dict) and set(executable) == EXECUTABLE_FIELDS,
        "runner.executable fields are invalid",
    )
    require(
        executable["git_identity"] == "excluded",
        "runner.executable.git_identity must be 'excluded'",
    )
    length = executable["length"]
    require(
        isinstance(length, int) and not isinstance(length, bool)
        and 0 < length <= MAX_ARTIFACT_BYTES,
        "runner.executable.length is invalid",
    )
    expected_hash = validate_sha256(
        executable["sha256"], "runner.executable.sha256",
    )
    executable_path = validate_local_regular_file(
        executable["path"], "runner.executable", root,
    )
    executable_length, executable_hash, _ = regular_file_measurement(executable_path)
    require(executable_length == length, "runner.executable length drift")
    require(executable_hash == expected_hash, "runner.executable SHA-256 drift")
    require_native_executable(executable_path, "runner.executable")
    validate_local_directory(
        runner["working_directory"], "runner.working_directory", root,
    )
    release_gate_cycle = runner["release_gate_cycle"]
    require(
        isinstance(release_gate_cycle, int)
        and not isinstance(release_gate_cycle, bool)
        and 0 <= release_gate_cycle <= (1 << 64) - 1,
        "runner.release_gate_cycle must be a nonnegative u64 integer",
    )

    argv = runner["argv"]
    require(isinstance(argv, list), "runner.argv must be an array")
    require(
        all(isinstance(argument, str) and argument and "\0" not in argument
            for argument in argv),
        "runner.argv entries must be nonempty strings without NUL",
    )
    environment = runner["env"]
    require(isinstance(environment, dict), "runner.env must be an object")
    for name, value in environment.items():
        require(
            isinstance(name, str) and ENV_NAME_RE.fullmatch(name) is not None,
            f"runner.env name {name!r} is invalid",
        )
        require(
            name not in RESERVED_RUNNER_ENV
            and not name.startswith("FN64_RELEASE_")
            and not name.startswith("FN64_PRIVATE_RUN_")
            and not name.startswith("OOT_RELEASE_"),
            f"runner.env name {name!r} is reserved for the trusted runner",
        )
        require(
            name not in FORBIDDEN_RUNNER_ENV
            and not name.startswith(FORBIDDEN_RUNNER_ENV_PREFIXES),
            f"runner.env name {name!r} can inject or replace child process code",
        )
        require(
            isinstance(value, str) and "\0" not in value,
            f"runner.env[{name!r}] must be a string without NUL",
        )
    validate_execution_source(runner["execution_source"], lane, "runner.execution_source")
    receipt_descriptor = runner["program_build_receipt"]
    if lane == "no_program_fixture":
        require(
            receipt_descriptor is None,
            "no_program_fixture runner cannot bind a program-build receipt",
        )
    else:
        require(
            isinstance(receipt_descriptor, dict)
            and set(receipt_descriptor) == EXECUTABLE_FIELDS,
            "runner.program_build_receipt fields are invalid",
        )
        require(
            receipt_descriptor["git_identity"] == "excluded",
            "runner.program_build_receipt.git_identity must be 'excluded'",
        )
        receipt_length = receipt_descriptor["length"]
        require(
            isinstance(receipt_length, int) and not isinstance(receipt_length, bool)
            and 0 < receipt_length <= MAX_ARTIFACT_BYTES,
            "runner.program_build_receipt.length is invalid",
        )
        receipt_sha256 = validate_sha256(
            receipt_descriptor["sha256"],
            "runner.program_build_receipt.sha256",
        )
        receipt_path = validate_local_regular_file(
            receipt_descriptor["path"], "runner.program_build_receipt", root,
        )
        observed_length, observed_sha256, _ = regular_file_measurement(receipt_path)
        require(
            observed_length == receipt_length,
            "runner.program_build_receipt length drift",
        )
        require(
            observed_sha256 == receipt_sha256,
            "runner.program_build_receipt SHA-256 drift",
        )
        require(
            recompiled_descriptor is not None,
            "authoritative program lane requires artifacts.recompiled",
        )
        normalized_recompiled = {
            "path": recompiled_descriptor["path"],
            "bytes": recompiled_descriptor["length"],
            "sha256": recompiled_descriptor["sha256"],
        }
        validate_program_build_receipt(
            load_json(receipt_path),
            receipt_path,
            lane,
            root,
            executable,
            runner["execution_source"],
            (normalized_recompiled, Path(recompiled_descriptor["path"])),
        )
        final_length, final_sha256, _ = regular_file_measurement(receipt_path)
        require(
            final_length == receipt_length and final_sha256 == receipt_sha256,
            "runner.program_build_receipt changed during validation",
        )
    return runner, executable_path


def validate_manifest(manifest: dict, manifest_path: Path, root: Path) -> tuple[dict, dict[str, Path]]:
    require(set(manifest) == ROOT_FIELDS, "manifest has unknown or missing root fields")
    schema = manifest["schema"]
    require(
        schema in {MANIFEST_SCHEMA, LEGACY_MANIFEST_SCHEMA},
        f"schema must be {MANIFEST_SCHEMA!r} or retained {LEGACY_MANIFEST_SCHEMA!r}",
    )
    validate_local_regular_file(str(manifest_path), "manifest", root)
    purpose = manifest["purpose"]
    allowed_purposes = PURPOSES if schema == MANIFEST_SCHEMA else LEGACY_PURPOSES
    require(
        purpose in allowed_purposes,
        f"purpose must be one of {sorted(allowed_purposes)} for {schema}",
    )

    intent = manifest["intent"]
    require(isinstance(intent, dict) and set(intent) == INTENT_FIELDS, "intent fields are invalid")
    wire_family = nonempty(intent["wire_family"], "intent.wire_family")
    allowed_wire_families = (
        WIRE_FAMILIES if schema == MANIFEST_SCHEMA else LEGACY_WIRE_FAMILIES
    )
    require(
        wire_family in allowed_wire_families,
        f"unsupported wire family {wire_family!r} for {schema}",
    )
    scenario = nonempty(intent["report_scenario"], "intent.report_scenario")
    require(
        SCENARIO_RE.fullmatch(scenario) is not None
        and re.fullmatch(r"[0-9a-f]{64}", scenario) is None,
        "intent.report_scenario is invalid",
    )
    require(
        intent["recognition"] == "runtime_must_confirm_backend_known_pair",
        "intent.recognition must preserve the exact backend text/data-pair gate",
    )
    program_lane = nonempty(intent["program_evidence_lane"], "intent.program_evidence_lane")
    rom_class = nonempty(intent["rom_class"], "intent.rom_class")
    if program_lane == "typed_function":
        raise AdmissionError(
            "intent.program_evidence_lane='typed_function' is not release-admissible: "
            "the lane name does not assert the generated entry-observation schema; install "
            "that schema and select 'typed_observed_function'"
        )
    if program_lane == "unidentified_native":
        raise AdmissionError(
            "intent.program_evidence_lane='unidentified_native' is not release-admissible: "
            "bind the exact linked archive identity and select 'identified_native_archive'"
        )
    require(
        program_lane in PROGRAM_EVIDENCE_LANES,
        f"intent.program_evidence_lane must be one of {sorted(PROGRAM_EVIDENCE_LANES)}",
    )
    cases = set(unique_strings(intent["extended_gbi_cases"], "intent.extended_gbi_cases"))
    if purpose in {"extended_gbi", "combined"}:
        require(wire_family == "f3dex2_extended_gbi_v1", "Extended GBI requires f3dex2_extended_gbi_v1")
        require(cases == EXTENDED_CASES, f"Extended GBI case denominator drifted: missing={sorted(EXTENDED_CASES - cases)}, extra={sorted(cases - EXTENDED_CASES)}")
    else:
        require(not cases, f"{purpose} admission must not claim Extended GBI cases")
    if purpose == "f3dzex2_characterization":
        require(
            wire_family == "f3dzex2",
            "F3DZEX2 characterization requires wire family f3dzex2",
        )

    release = manifest["release_matrix"]
    require(isinstance(release, dict) and set(release) == RELEASE_FIELDS, "release_matrix fields are invalid")
    require(release["platform"] in PLATFORMS, "release_matrix.platform is invalid")
    controllers = set(unique_strings(release["controllers"], "release_matrix.controllers"))
    require(bool(controllers) and controllers <= CONTROLLERS, "release_matrix.controllers is invalid")
    require(release["save"] in SAVES, "release_matrix.save is invalid")
    renderers = set(unique_strings(release["renderers"], "release_matrix.renderers"))
    require(bool(renderers) and renderers <= RENDERERS, "release_matrix.renderers is invalid")
    if "reference_lle_accuracy" in renderers:
        require(renderers == {"reference_lle_accuracy"}, "reference LLE must stand alone")
    else:
        require("rt64_lle_accuracy" in renderers, "RT64 renderer coverage requires rt64_lle_accuracy")
    if purpose in {"extended_gbi", "combined"}:
        require({"rt64_lle_accuracy", "rt64_post_vi_capture"} <= renderers, "Extended GBI requires RT64 LLE and post-VI capture coverage")
    if purpose == "f3dzex2_characterization":
        require(
            {"rt64_lle_accuracy", "rt64_post_vi_capture"} <= renderers,
            "F3DZEX2 characterization requires RT64 LLE and post-VI capture coverage",
        )
    require(release["repeat_bar"] == 10, "release_matrix.repeat_bar must be exactly 10")

    artifacts = manifest["artifacts"]
    artifact_fields = (
        ARTIFACT_FIELDS if schema == MANIFEST_SCHEMA else LEGACY_ARTIFACT_FIELDS
    )
    require(
        isinstance(artifacts, dict) and set(artifacts) == artifact_fields,
        f"artifacts fields are invalid for {schema}",
    )
    admitted: dict[str, Path] = {}
    required_roles = (
        ("microcode_text_raw_window", "microcode_data_raw_window")
        if purpose == "f3dzex2_characterization"
        else ("microcode_text", "microcode_data")
    )
    for required_role in required_roles:
        require(artifacts[required_role] is not None, f"artifacts.{required_role} is required")
    for role in artifact_fields:
        descriptor = artifacts[role]
        if descriptor is not None:
            admitted[role] = validate_artifact(role, descriptor, root)
    if purpose == "f3dzex2_characterization":
        require(
            set(admitted)
            == {"microcode_text_raw_window", "microcode_data_raw_window"},
            "F3DZEX2 characterization admits exactly the two native raw recognition windows",
        )
    else:
        require(
            not ({"microcode_text_raw_window", "microcode_data_raw_window"} & set(admitted)),
            f"{purpose} admission cannot mix logical microcode artifacts with raw characterization windows",
        )
        require(
            artifacts["microcode_text"]["length"] == 4096,
            "artifacts.microcode_text must contain the exact 4096-byte RSP IMEM image",
        )
        require(
            artifacts["microcode_data"]["length"] <= (1 << 32) - 1,
            "artifacts.microcode_data length exceeds the task-header u32 size field",
        )
    if purpose in {"full_rom", "combined"}:
        require({"rom", "recompiled"} <= set(admitted), f"{purpose} admission requires ROM and recompiled artifacts")
        require(
            rom_class in ROM_CLASSES,
            f"{purpose} admission requires intent.rom_class to be one of {sorted(ROM_CLASSES)}",
        )
        expected_provenance = ROM_PROVENANCE_BY_CLASS[rom_class]
        require(
            artifacts["rom"]["provenance"] == expected_provenance,
            f"artifacts.rom.provenance must be {expected_provenance!r} for ROM class {rom_class!r}",
        )
        require(
            program_lane in {
                "identified_native_archive", "typed_observed_function",
                "typed_block_program",
            },
            f"{purpose} admission requires an authoritative executable lane: "
            "'identified_native_archive', 'typed_observed_function', or "
            "'typed_block_program'",
        )
    elif purpose == "extended_gbi":
        require(
            rom_class == "not_applicable",
            "extended_gbi-only admission must use intent.rom_class='not_applicable'",
        )
        require(
            program_lane == "no_program_fixture",
            "extended_gbi-only admission must select 'no_program_fixture'; executable full-ROM "
            "lane claims require purpose 'full_rom' or 'combined'",
        )
    else:
        require(
            rom_class == "not_applicable",
            "F3DZEX2 characterization must use intent.rom_class='not_applicable'",
        )
        require(
            program_lane == "no_program_fixture",
            "F3DZEX2 characterization must select 'no_program_fixture'",
        )
    validate_runner(
        manifest["runner"],
        program_lane,
        root,
        artifacts.get("recompiled"),
    )

    readiness = {
        "schema": (
            READINESS_SCHEMA
            if schema == MANIFEST_SCHEMA
            else LEGACY_READINESS_SCHEMA
        ),
        "status": "ready",
        "purpose": purpose,
        "wire_family": wire_family,
        "report_scenario": scenario,
        "rom_class": rom_class,
        "program_evidence_lane": program_lane,
        "artifact_roles_admitted": sorted(admitted),
        "extended_gbi_fixture": "ready_for_runtime_recognition" if purpose in {"extended_gbi", "combined"} else "not_requested",
        "full_rom_inputs": "ready" if {"rom", "recompiled"} <= set(admitted) else "not_supplied",
        "program_build_receipt": (
            "verified" if program_lane != "no_program_fixture" else "not_applicable"
        ),
        "release_matrix_policy": "ready_for_ten_run_evidence",
        "repeat_bar": 10,
        "required_extended_cases": sorted(cases),
        "platform": release["platform"],
        "controllers": sorted(controllers),
        "save": release["save"],
        "renderers": sorted(renderers),
    }
    return readiness, admitted


def validate_readiness(report: dict) -> None:
    require(set(report) == READINESS_FIELDS, "readiness report has unknown or missing fields")
    schema = report["schema"]
    require(
        schema in {READINESS_SCHEMA, LEGACY_READINESS_SCHEMA},
        f"readiness schema must be {READINESS_SCHEMA!r} or retained {LEGACY_READINESS_SCHEMA!r}",
    )
    require(report["status"] == "ready", "readiness status must be ready")
    purpose = report["purpose"]
    allowed_purposes = PURPOSES if schema == READINESS_SCHEMA else LEGACY_PURPOSES
    require(purpose in allowed_purposes, "readiness purpose is invalid")
    allowed_wire_families = (
        WIRE_FAMILIES if schema == READINESS_SCHEMA else LEGACY_WIRE_FAMILIES
    )
    require(
        report["wire_family"] in allowed_wire_families,
        "readiness wire family is invalid",
    )
    scenario = nonempty(report["report_scenario"], "readiness report_scenario")
    rom_class = nonempty(report["rom_class"], "readiness rom_class")
    require(SCENARIO_RE.fullmatch(scenario) is not None and re.fullmatch(r"[0-9a-f]{64}", scenario) is None, "readiness report_scenario is invalid")
    roles = set(unique_strings(report["artifact_roles_admitted"], "readiness artifact_roles_admitted"))
    program_lane = nonempty(report["program_evidence_lane"], "readiness program_evidence_lane")
    require(program_lane in PROGRAM_EVIDENCE_LANES, "readiness program-evidence lane is invalid")
    artifact_fields = (
        ARTIFACT_FIELDS if schema == READINESS_SCHEMA else LEGACY_ARTIFACT_FIELDS
    )
    require(roles <= artifact_fields, "readiness artifact roles are invalid")
    if purpose == "f3dzex2_characterization":
        require(
            roles == {"microcode_text_raw_window", "microcode_data_raw_window"},
            "readiness F3DZEX2 characterization roles are incomplete or ambiguous",
        )
    else:
        require(
            {"microcode_text", "microcode_data"} <= roles,
            "readiness logical microcode artifact roles are incomplete",
        )
        require(
            not ({"microcode_text_raw_window", "microcode_data_raw_window"} & roles),
            "readiness non-characterization purpose claims raw recognition windows",
        )
    require(report["extended_gbi_fixture"] in {"ready_for_runtime_recognition", "not_requested"}, "readiness Extended GBI state is invalid")
    require(report["full_rom_inputs"] in {"ready", "not_supplied"}, "readiness full-ROM state is invalid")
    require(
        report["program_build_receipt"] in {"verified", "not_applicable"},
        "readiness program-build receipt state is invalid",
    )
    require(report["release_matrix_policy"] == "ready_for_ten_run_evidence", "readiness release-matrix state is invalid")
    require(report["repeat_bar"] == 10, "readiness repeat bar is invalid")
    require(report["platform"] in PLATFORMS, "readiness platform is invalid")
    controllers = set(unique_strings(report["controllers"], "readiness controllers"))
    require(bool(controllers) and controllers <= CONTROLLERS, "readiness controllers are invalid")
    require(report["save"] in SAVES, "readiness save is invalid")
    renderers = set(unique_strings(report["renderers"], "readiness renderers"))
    require(bool(renderers) and renderers <= RENDERERS, "readiness renderers are invalid")
    if "reference_lle_accuracy" in renderers:
        require(renderers == {"reference_lle_accuracy"}, "readiness reference LLE must stand alone")
    else:
        require("rt64_lle_accuracy" in renderers, "readiness RT64 policy lacks rt64_lle_accuracy")
    cases = set(unique_strings(report["required_extended_cases"], "readiness required_extended_cases"))
    require(cases in (set(), EXTENDED_CASES), "readiness Extended GBI denominator is invalid")
    if report["purpose"] in {"extended_gbi", "combined"}:
        require(report["extended_gbi_fixture"] == "ready_for_runtime_recognition" and cases == EXTENDED_CASES, "readiness Extended GBI state is inconsistent")
        require({"rt64_lle_accuracy", "rt64_post_vi_capture"} <= renderers, "readiness Extended GBI renderer policy is incomplete")
    else:
        require(report["extended_gbi_fixture"] == "not_requested" and not cases, f"readiness {purpose} report claims Extended GBI")
    if purpose == "f3dzex2_characterization":
        require(
            {"rt64_lle_accuracy", "rt64_post_vi_capture"} <= renderers,
            "readiness F3DZEX2 characterization renderer policy is incomplete",
        )
    if report["purpose"] in {"full_rom", "combined"}:
        require(rom_class in ROM_CLASSES, "readiness full-ROM ROM class is invalid")
        require(report["full_rom_inputs"] == "ready" and {"rom", "recompiled"} <= roles, "readiness full-ROM inputs are incomplete")
        require(
            report["program_build_receipt"] == "verified",
            "readiness full-ROM program-build receipt is not verified",
        )
        require(
            program_lane in {
                "identified_native_archive", "typed_observed_function",
                "typed_block_program",
            },
            "readiness full-ROM program-evidence lane is not authoritative",
        )
    else:
        require(
            rom_class == "not_applicable",
            "readiness fixture ROM class must be not_applicable",
        )
        require(
            report["program_build_receipt"] == "not_applicable",
            "readiness fixture cannot claim a program-build receipt",
        )
        require(program_lane == "no_program_fixture", "readiness fixture program lane is inconsistent")
    forbidden = {"path", "sha256", "hash", "length", "bytes", "filename", "manifest"}
    for key in report:
        require(not any(token in key.lower() for token in forbidden), f"readiness field {key!r} leaks private identity")


def validate_output_path(path: Path, root: Path, option: str = "--report") -> None:
    require(path.is_absolute(), f"{option} must be absolute")
    require(".." not in path.parts, f"{option} must not contain '..'")
    reject_symlink_components(path, include_leaf=False)
    require(
        not path.exists() and not path.is_symlink(),
        f"refusing to overwrite output {path}",
    )
    require(
        git_excluded(path, root),
        f"{option} is inside the repository and not gitignored",
    )


def serialize_json_document(value: dict) -> bytes:
    return (json.dumps(value, indent=2) + "\n").encode("utf-8")


def publish_complete_file_exclusive(
    path: Path,
    payload: bytes,
    root: Path,
    option: str,
) -> None:
    validate_output_path(path, root, option)
    path.parent.mkdir(parents=True, exist_ok=True)
    # mkdir and the first policy check operate by pathname. Recheck immediately
    # before pinning the parent directory and creating the staged inode.
    reject_symlink_components(path, include_leaf=False)
    validate_output_path(path, root, option)

    parent_descriptor: int | None = None
    temporary_name: str | None = None
    temporary_path: Path | None = None
    try:
        if secure_publish_dir_fd_available():
            parent_descriptor = open_directory_nofollow(path.parent)
            for _ in range(32):
                candidate = f".{path.name}.fn64-stage-{secrets.token_hex(16)}"
                try:
                    descriptor = os.open(
                        candidate,
                        os.O_WRONLY | os.O_CREAT | os.O_EXCL
                        | getattr(os, "O_CLOEXEC", 0)
                        | getattr(os, "O_NOFOLLOW", 0),
                        0o600,
                        dir_fd=parent_descriptor,
                    )
                    temporary_name = candidate
                    break
                except FileExistsError:
                    continue
            else:
                raise AdmissionError(f"cannot allocate a staging file for {path}")
        else:
            for _ in range(32):
                temporary_path = path.with_name(
                    f".{path.name}.fn64-stage-{secrets.token_hex(16)}"
                )
                try:
                    descriptor = open_regular_nofollow(
                        temporary_path,
                        os.O_WRONLY | os.O_CREAT | os.O_EXCL,
                    )
                    break
                except FileExistsError:
                    continue
            else:
                raise AdmissionError(f"cannot allocate a staging file for {path}")

        try:
            with os.fdopen(descriptor, "wb", closefd=False) as file:
                file.write(payload)
                file.flush()
                os.fsync(descriptor)
            retained = os.fstat(descriptor)
            require(
                stat.S_ISREG(retained.st_mode) and retained.st_size == len(payload),
                f"staged output verification failed for {path}",
            )
        finally:
            os.close(descriptor)

        try:
            if parent_descriptor is not None:
                require(temporary_name is not None, "internal staging-name error")
                os.link(
                    temporary_name,
                    path.name,
                    src_dir_fd=parent_descriptor,
                    dst_dir_fd=parent_descriptor,
                    follow_symlinks=False,
                )
                os.unlink(temporary_name, dir_fd=parent_descriptor)
                temporary_name = None
                try:
                    os.fsync(parent_descriptor)
                except OSError:
                    pass
            else:
                require(temporary_path is not None, "internal staging-path error")
                if os.name == "nt":
                    os.rename(temporary_path, path)
                else:
                    os.link(temporary_path, path, follow_symlinks=False)
                    temporary_path.unlink()
                temporary_path = None
        except FileExistsError as error:
            raise AdmissionError(f"refusing to overwrite output {path}") from error
        except OSError as error:
            if error.errno == errno.EEXIST:
                raise AdmissionError(f"refusing to overwrite output {path}") from error
            raise AdmissionError(f"cannot publish complete output {path}: {error}") from error
    finally:
        if temporary_name is not None and parent_descriptor is not None:
            try:
                os.unlink(temporary_name, dir_fd=parent_descriptor)
            except FileNotFoundError:
                pass
        if parent_descriptor is not None:
            os.close(parent_descriptor)
        if temporary_path is not None:
            try:
                temporary_path.unlink()
            except FileNotFoundError:
                pass


def write_report(path: Path, report: dict, root: Path) -> None:
    validate_readiness(report)
    publish_complete_file_exclusive(
        path,
        serialize_json_document(report),
        root,
        "--report",
    )


def append_u64(wire: bytearray, value: object, where: str) -> None:
    require(
        isinstance(value, int) and not isinstance(value, bool)
        and 0 <= value <= (1 << 64) - 1,
        f"{where} must be a u64 integer",
    )
    wire.extend(value.to_bytes(8, "big"))


def append_string(wire: bytearray, value: object, where: str) -> None:
    require(isinstance(value, str), f"{where} must be a string")
    encoded = value.encode("utf-8")
    append_u64(wire, len(encoded), f"{where} encoded length")
    wire.extend(encoded)


def append_sha256(wire: bytearray, value: object, where: str) -> None:
    wire.extend(bytes.fromhex(validate_sha256(value, where)))


def append_file_identity(wire: bytearray, value: object, where: str) -> None:
    require(
        isinstance(value, dict) and set(value) == CONTRACT_DESCRIPTOR_FIELDS,
        f"{where} fields are invalid",
    )
    append_string(wire, value["path"], f"{where}.path")
    append_u64(wire, value["bytes"], f"{where}.bytes")
    append_sha256(wire, value["sha256"], f"{where}.sha256")


def append_artifact_identity(wire: bytearray, value: object, where: str) -> None:
    require(
        isinstance(value, dict)
        and set(value) == CONTRACT_INPUT_DESCRIPTOR_FIELDS,
        f"{where} fields are invalid",
    )
    append_string(wire, value["role"], f"{where}.role")
    append_string(wire, value["path"], f"{where}.path")
    append_u64(wire, value["bytes"], f"{where}.bytes")
    append_sha256(wire, value["sha256"], f"{where}.sha256")
    append_string(wire, value["provenance"], f"{where}.provenance")


def append_execution_source(wire: bytearray, value: object, where: str) -> None:
    require(isinstance(value, dict), f"{where} must be an object")
    kind = nonempty(value.get("kind"), f"{where}.kind")
    require(kind in EXECUTION_SOURCE_FIELDS, f"{where}.kind is invalid")
    require(
        set(value) == EXECUTION_SOURCE_FIELDS[kind],
        f"{where} fields are invalid for {kind!r}",
    )
    tag = {
        "no_program": 0,
        "native_archive": 1,
        "typed_observed_function_program": 2,
        "typed_block_program": 3,
    }[kind]
    wire.append(tag)
    if kind in {"native_archive", "typed_observed_function_program"}:
        append_sha256(wire, value["artifact_sha256"], f"{where}.artifact_sha256")
    elif kind == "typed_block_program":
        append_sha256(wire, value["program_sha256"], f"{where}.program_sha256")
        append_sha256(
            wire,
            value["dispatch_artifact_sha256"],
            f"{where}.dispatch_artifact_sha256",
        )


def append_program_build_lane(wire: bytearray, value: object, where: str) -> None:
    require(isinstance(value, dict), f"{where} must be an object")
    kind = nonempty(value.get("kind"), f"{where}.kind")
    require(kind in PROGRAM_BUILD_LANE_FIELDS, f"{where}.kind is invalid")
    require(
        set(value) == PROGRAM_BUILD_LANE_FIELDS[kind],
        f"{where} fields are invalid for {kind!r}",
    )
    if kind == "native_archives":
        wire.append(1)
        archives = value["archives"]
        require(isinstance(archives, list), f"{where}.archives must be an array")
        append_u64(wire, len(archives), f"{where}.archives count")
        for index, archive in enumerate(archives):
            require(
                isinstance(archive, dict)
                and set(archive) == NATIVE_ARCHIVE_INPUT_FIELDS,
                f"{where}.archives[{index}] fields are invalid",
            )
            append_string(
                wire, archive["label"], f"{where}.archives[{index}].label",
            )
            append_file_identity(
                wire, archive["file"], f"{where}.archives[{index}].file",
            )
    elif kind == "typed_observed_function":
        wire.append(2)
        append_file_identity(wire, value["identity_wire"], f"{where}.identity_wire")
    else:
        wire.append(3)
        append_file_identity(wire, value["pack"], f"{where}.pack")
        append_sha256(
            wire,
            value["expected_program_sha256"],
            f"{where}.expected_program_sha256",
        )


def program_build_receipt_sha256(receipt_without_sha256: dict) -> str:
    wire = bytearray(PROGRAM_BUILD_RECEIPT_DIGEST_DOMAIN)
    append_string(wire, receipt_without_sha256["schema"], "program receipt.schema")
    append_file_identity(
        wire,
        receipt_without_sha256["child_executable"],
        "program receipt.child_executable",
    )
    append_program_build_lane(
        wire, receipt_without_sha256["lane"], "program receipt.lane",
    )
    append_execution_source(
        wire,
        receipt_without_sha256["expected_execution_source"],
        "program receipt.expected_execution_source",
    )
    return hashlib.sha256(wire).hexdigest()


def validate_program_file_identity(
    value: object, where: str, root: Path,
) -> tuple[dict, Path]:
    require(
        isinstance(value, dict) and set(value) == PROGRAM_FILE_IDENTITY_FIELDS,
        f"{where} fields are invalid",
    )
    path = validate_local_regular_file(value["path"], where, root)
    length = value["bytes"]
    require(
        isinstance(length, int) and not isinstance(length, bool)
        and 0 < length <= MAX_ARTIFACT_BYTES,
        f"{where}.bytes is invalid",
    )
    expected_sha256 = validate_sha256(value["sha256"], f"{where}.sha256")
    observed_length, observed_sha256, _ = regular_file_measurement(path)
    require(observed_length == length, f"{where} length drift")
    require(observed_sha256 == expected_sha256, f"{where} SHA-256 drift")
    return value, path


def native_archive_source_sha256(
    archives: list[tuple[str, dict, Path]],
) -> str:
    digest = hashlib.sha256()
    digest.update(b"fn64.native-program-archives.v1\0")
    digest.update(len(archives).to_bytes(8, "big"))
    for label, identity, path in archives:
        encoded_label = label.encode("utf-8")
        digest.update(len(encoded_label).to_bytes(8, "big"))
        digest.update(encoded_label)
        digest.update(identity["bytes"].to_bytes(8, "big"))
        descriptor = open_regular_nofollow(path)
        try:
            before = os.fstat(descriptor)
            require(stat.S_ISREG(before.st_mode), f"{path} must remain a regular file")
            archive_digest = hashlib.sha256()
            observed_bytes = 0
            with os.fdopen(descriptor, "rb", closefd=False) as file:
                while block := file.read(1024 * 1024):
                    digest.update(block)
                    archive_digest.update(block)
                    observed_bytes += len(block)
            after = os.fstat(descriptor)
            require(
                stable_file_identity(before) == stable_file_identity(after),
                f"{path} changed while computing native program identity",
            )
            require(
                observed_bytes == identity["bytes"]
                and before.st_size == identity["bytes"],
                f"{path} length drift",
            )
            require(
                archive_digest.hexdigest() == identity["sha256"],
                f"{path} SHA-256 drift while computing native program identity",
            )
        finally:
            os.close(descriptor)
    return digest.hexdigest()


def validate_program_build_receipt(
    receipt: dict,
    receipt_path: Path,
    lane: str,
    root: Path,
    expected_child: dict,
    expected_execution_source: dict,
    recompiled: tuple[dict, Path],
) -> None:
    require(
        set(receipt) == PROGRAM_BUILD_RECEIPT_FIELDS,
        "program-build receipt has unknown or missing fields",
    )
    require(
        receipt["schema"] == PROGRAM_BUILD_RECEIPT_SCHEMA,
        f"program-build receipt schema must be {PROGRAM_BUILD_RECEIPT_SCHEMA!r}",
    )
    child, child_path = validate_program_file_identity(
        receipt["child_executable"], "program receipt.child_executable", root,
    )
    require(
        child["path"] == expected_child["path"]
        and child["bytes"] == expected_child["length"]
        and child["sha256"] == expected_child["sha256"],
        "program-build receipt child does not match runner.executable",
    )
    require_native_executable(child_path, "program receipt.child_executable")

    lane_value = receipt["lane"]
    require(isinstance(lane_value, dict), "program receipt.lane must be an object")
    kind = nonempty(lane_value.get("kind"), "program receipt.lane.kind")
    expected_kind = {
        "identified_native_archive": "native_archives",
        "typed_observed_function": "typed_observed_function",
        "typed_block_program": "typed_block",
    }.get(lane)
    require(kind == expected_kind, "program-build receipt lane does not match manifest lane")
    append_program_build_lane(bytearray(), lane_value, "program receipt.lane")

    recompiled_value, _ = recompiled
    matching_recompiled = 0
    if kind == "native_archives":
        raw_archives = lane_value["archives"]
        require(bool(raw_archives), "program receipt native archive list is empty")
        archives: list[tuple[str, dict, Path]] = []
        labels: list[str] = []
        for index, archive in enumerate(raw_archives):
            require(
                isinstance(archive, dict)
                and set(archive) == NATIVE_ARCHIVE_INPUT_FIELDS,
                f"program receipt.lane.archives[{index}] fields are invalid",
            )
            label = nonempty(
                archive["label"], f"program receipt.lane.archives[{index}].label",
            )
            require(
                re.fullmatch(r"[a-z0-9][a-z0-9._-]{0,127}", label) is not None,
                f"program receipt archive label {label!r} is not canonical",
            )
            identity, path = validate_program_file_identity(
                archive["file"], f"program receipt.lane.archives[{index}].file", root,
            )
            labels.append(label)
            archives.append((label, identity, path))
            matching_recompiled += int(
                identity["path"] == recompiled_value["path"]
                and identity["bytes"] == recompiled_value["bytes"]
                and identity["sha256"] == recompiled_value["sha256"]
            )
        require(labels == sorted(labels), "program receipt archive labels are not sorted")
        require(len(labels) == len(set(labels)), "program receipt archive labels repeat")
        recomputed_source = {
            "kind": "native_archive",
            "artifact_sha256": native_archive_source_sha256(archives),
        }
    elif kind == "typed_observed_function":
        identity, _ = validate_program_file_identity(
            lane_value["identity_wire"], "program receipt.lane.identity_wire", root,
        )
        matching_recompiled = int(
            identity["path"] == recompiled_value["path"]
            and identity["bytes"] == recompiled_value["bytes"]
            and identity["sha256"] == recompiled_value["sha256"]
        )
        recomputed_source = {
            "kind": "typed_observed_function_program",
            "artifact_sha256": identity["sha256"],
        }
    else:
        identity, _ = validate_program_file_identity(
            lane_value["pack"], "program receipt.lane.pack", root,
        )
        expected_program = validate_sha256(
            lane_value["expected_program_sha256"],
            "program receipt.lane.expected_program_sha256",
        )
        matching_recompiled = int(
            identity["path"] == recompiled_value["path"]
            and identity["bytes"] == recompiled_value["bytes"]
            and identity["sha256"] == recompiled_value["sha256"]
        )
        recomputed_source = {
            "kind": "typed_block_program",
            "program_sha256": expected_program,
            "dispatch_artifact_sha256": identity["sha256"],
        }
    require(
        matching_recompiled == 1,
        "program-build receipt must bind exactly one lane input equal to artifacts.recompiled",
    )
    declared_source = receipt["expected_execution_source"]
    validate_execution_source(
        declared_source, lane, "program receipt.expected_execution_source",
    )
    require(
        declared_source == recomputed_source
        and declared_source == expected_execution_source,
        "program-build receipt execution source does not match recomputed and runner identities",
    )
    claimed = validate_sha256(
        receipt["receipt_sha256"], "program receipt.receipt_sha256",
    )
    unsigned = {
        key: value for key, value in receipt.items() if key != "receipt_sha256"
    }
    require(
        claimed == program_build_receipt_sha256(unsigned),
        "program-build receipt canonical SHA-256 drift",
    )
    # Recheck its own bound bytes after all referenced files have been read.
    regular_file_measurement(receipt_path)


def private_run_contract_sha256(contract_without_sha256: dict) -> str:
    wire = bytearray(PRIVATE_RUN_CONTRACT_DIGEST_DOMAIN)
    append_string(wire, contract_without_sha256["schema"], "contract.schema")
    append_file_identity(
        wire,
        contract_without_sha256["admission_manifest"],
        "contract.admission_manifest",
    )
    append_file_identity(
        wire,
        contract_without_sha256["readiness_report"],
        "contract.readiness_report",
    )
    program_receipt = contract_without_sha256["program_build_receipt"]
    if program_receipt is None:
        wire.append(0)
    else:
        wire.append(1)
        append_file_identity(
            wire, program_receipt, "contract.program_build_receipt",
        )
    append_string(wire, contract_without_sha256["purpose"], "contract.purpose")
    append_string(
        wire,
        contract_without_sha256["rom_class"],
        "contract.rom_class",
    )
    append_string(
        wire,
        contract_without_sha256["report_scenario"],
        "contract.report_scenario",
    )
    append_u64(wire, contract_without_sha256["guest_cycle"], "contract.guest_cycle")
    append_u64(wire, contract_without_sha256["repeat_count"], "contract.repeat_count")
    append_artifact_identity(wire, contract_without_sha256["input"], "contract.input")
    artifacts = contract_without_sha256["admitted_artifacts"]
    require(isinstance(artifacts, list), "contract.admitted_artifacts must be an array")
    append_u64(wire, len(artifacts), "contract.admitted_artifacts count")
    for index, artifact in enumerate(artifacts):
        append_artifact_identity(
            wire, artifact, f"contract.admitted_artifacts[{index}]",
        )
    append_execution_source(
        wire,
        contract_without_sha256["expected_execution_source"],
        "contract.expected_execution_source",
    )
    child = contract_without_sha256["child"]
    require(
        isinstance(child, dict) and set(child) == CONTRACT_CHILD_FIELDS,
        "contract.child fields are invalid",
    )
    append_file_identity(wire, child["executable"], "contract.child.executable")
    append_string(
        wire, child["working_directory"], "contract.child.working_directory",
    )
    argv = child["argv"]
    require(isinstance(argv, list), "contract.child.argv must be an array")
    append_u64(wire, len(argv), "contract.child.argv count")
    for index, argument in enumerate(argv):
        append_string(wire, argument, f"contract.child.argv[{index}]")
    environment = child["environment"]
    require(isinstance(environment, list), "contract.child.environment must be an array")
    append_u64(wire, len(environment), "contract.child.environment count")
    for index, entry in enumerate(environment):
        require(
            isinstance(entry, dict) and set(entry) == CONTRACT_ENVIRONMENT_FIELDS,
            f"contract.child.environment[{index}] fields are invalid",
        )
        append_string(wire, entry["name"], f"contract.child.environment[{index}].name")
        append_string(wire, entry["value"], f"contract.child.environment[{index}].value")
    return hashlib.sha256(wire).hexdigest()


def contract_descriptor(path: Path) -> dict:
    length, digest, _ = regular_file_measurement(path)
    return {
        "path": str(path),
        "bytes": length,
        "sha256": digest,
    }


def contract_descriptor_for_bytes(path: Path, payload: bytes) -> dict:
    return {
        "path": str(path),
        "bytes": len(payload),
        "sha256": hashlib.sha256(payload).hexdigest(),
    }


def contract_artifact_descriptor(role: str, path: Path, manifest: dict) -> dict:
    length, digest, _ = regular_file_measurement(path)
    return {
        "role": role,
        "path": str(path),
        "bytes": length,
        "sha256": digest,
        "provenance": manifest["artifacts"][role]["provenance"],
    }


def build_private_run_contract(
    manifest: dict,
    manifest_path: Path,
    readiness_path: Path,
    admitted: dict[str, Path],
    readiness_payload: bytes | None = None,
) -> dict:
    require(
        manifest["purpose"] in {"full_rom", "combined"},
        "private run-contract emission requires purpose full_rom or combined",
    )
    runner = manifest["runner"]
    executable_path = Path(runner["executable"]["path"])
    program_receipt_path = Path(runner["program_build_receipt"]["path"])
    require("rom" in admitted, "private run contract requires an admitted ROM input")
    unsigned = {
        "schema": PRIVATE_RUN_CONTRACT_SCHEMA,
        "admission_manifest": contract_descriptor(manifest_path),
        "readiness_report": (
            contract_descriptor(readiness_path)
            if readiness_payload is None
            else contract_descriptor_for_bytes(readiness_path, readiness_payload)
        ),
        "program_build_receipt": contract_descriptor(program_receipt_path),
        "purpose": manifest["purpose"],
        "rom_class": manifest["intent"]["rom_class"],
        "report_scenario": manifest["intent"]["report_scenario"],
        "guest_cycle": runner["release_gate_cycle"],
        "repeat_count": 10,
        "input": contract_artifact_descriptor("rom", admitted["rom"], manifest),
        "admitted_artifacts": [
            contract_artifact_descriptor(role, path, manifest)
            for role, path in sorted(admitted.items())
            if role != "rom"
        ],
        "expected_execution_source": dict(runner["execution_source"]),
        "child": {
            "executable": contract_descriptor(executable_path),
            "working_directory": runner["working_directory"],
            "argv": list(runner["argv"]),
            "environment": [
                {"name": name, "value": value}
                for name, value in sorted(runner["env"].items())
            ],
        },
    }
    return {
        **unsigned,
        "contract_sha256": private_run_contract_sha256(unsigned),
    }


def validate_private_run_contract(
    contract: dict,
    contract_path: Path | None,
    root: Path,
    readiness_binding: tuple[Path, dict, bytes] | None = None,
) -> None:
    if contract_path is not None:
        validate_local_regular_file(str(contract_path), "private run contract", root)
    require(
        set(contract) == CONTRACT_FIELDS,
        "private run contract has unknown or missing fields",
    )
    require(
        contract["schema"] == PRIVATE_RUN_CONTRACT_SCHEMA,
        f"private run contract schema must be {PRIVATE_RUN_CONTRACT_SCHEMA!r}",
    )
    require(contract["repeat_count"] == 10, "contract repeat_count must be exactly 10")
    require(
        contract["purpose"] in {"full_rom", "combined"},
        "private run contract purpose must be full_rom or combined",
    )
    guest_cycle = contract["guest_cycle"]
    require(
        isinstance(guest_cycle, int) and not isinstance(guest_cycle, bool)
        and 0 <= guest_cycle <= (1 << 64) - 1,
        "contract guest_cycle must be a nonnegative u64 integer",
    )
    scenario = nonempty(contract["report_scenario"], "contract report_scenario")
    require(
        SCENARIO_RE.fullmatch(scenario) is not None
        and re.fullmatch(r"[0-9a-f]{64}", scenario) is None,
        "contract report_scenario is invalid",
    )
    _, manifest_path = validate_contract_descriptor(
        contract["admission_manifest"], "contract.admission_manifest", root,
    )
    manifest = load_json(manifest_path)
    readiness, admitted = validate_manifest(manifest, manifest_path, root)
    if readiness_binding is None:
        _, readiness_path = validate_contract_descriptor(
            contract["readiness_report"], "contract.readiness_report", root,
        )
        retained_readiness = load_json(readiness_path)
    else:
        readiness_path, retained_readiness, readiness_payload = readiness_binding
        descriptor = contract["readiness_report"]
        require(
            isinstance(descriptor, dict)
            and set(descriptor) == CONTRACT_DESCRIPTOR_FIELDS,
            "contract.readiness_report: invalid descriptor fields",
        )
        require(
            descriptor == contract_descriptor_for_bytes(
                readiness_path, readiness_payload,
            ),
            "contract readiness descriptor does not match its in-memory report",
        )
        require(
            serialize_json_document(retained_readiness) == readiness_payload,
            "in-memory readiness serialization drifted",
        )
    validate_readiness(retained_readiness)
    require(
        retained_readiness == readiness,
        "contract readiness report does not match its validated manifest",
    )
    program_receipt, program_receipt_path = validate_contract_descriptor(
        contract["program_build_receipt"],
        "contract.program_build_receipt",
        root,
    )
    manifest_program_receipt = manifest["runner"]["program_build_receipt"]
    require(
        isinstance(manifest_program_receipt, dict)
        and program_receipt_path == Path(manifest_program_receipt["path"])
        and program_receipt["bytes"] == manifest_program_receipt["length"]
        and program_receipt["sha256"] == manifest_program_receipt["sha256"],
        "contract program-build receipt does not match the validated manifest",
    )
    require(
        manifest["purpose"] == contract["purpose"]
        and manifest["intent"]["rom_class"] == contract["rom_class"]
        and manifest["intent"]["report_scenario"] == scenario
        and manifest["runner"]["release_gate_cycle"] == guest_cycle,
        "contract policy fields do not match the validated manifest",
    )
    require(
        contract["rom_class"] in ROM_CLASSES,
        "private run contract ROM class is invalid",
    )
    require(
        contract["input"]["provenance"]
        == ROM_PROVENANCE_BY_CLASS[contract["rom_class"]],
        "private run contract ROM provenance does not match its class",
    )
    lane = manifest["intent"]["program_evidence_lane"]
    expected_source = contract["expected_execution_source"]
    validate_execution_source(
        expected_source, lane, "contract.expected_execution_source",
    )
    require(
        expected_source == manifest["runner"]["execution_source"]
        and expected_source["kind"] != "no_program",
        "contract execution source does not match the authoritative manifest lane",
    )

    input_value, input_path = validate_contract_artifact_descriptor(
        contract["input"], "contract.input", root,
    )
    require(
        input_value["role"] == "rom"
        and input_path == admitted["rom"]
        and input_value["bytes"] == manifest["artifacts"]["rom"]["length"]
        and input_value["sha256"] == manifest["artifacts"]["rom"]["sha256"]
        and input_value["provenance"] == manifest["artifacts"]["rom"]["provenance"],
        "contract input does not match the admitted ROM descriptor",
    )
    artifacts = contract["admitted_artifacts"]
    require(isinstance(artifacts, list), "contract.admitted_artifacts must be an array")
    roles = [
        nonempty(value.get("role") if isinstance(value, dict) else None,
                 f"contract.admitted_artifacts[{index}].role")
        for index, value in enumerate(artifacts)
    ]
    require(roles == sorted(roles), "contract admitted artifact roles are not sorted")
    require(len(roles) == len(set(roles)), "contract admitted artifact roles repeat")
    expected_roles = set(admitted) - {"rom"}
    require(set(roles) == expected_roles, "contract admitted artifact roles drifted")
    for index, value in enumerate(artifacts):
        role = roles[index]
        retained, observed_path = validate_contract_artifact_descriptor(
            value, f"contract.admitted_artifacts[{index}]", root,
        )
        require(
            observed_path == admitted[role]
            and retained["bytes"] == manifest["artifacts"][role]["length"]
            and retained["sha256"] == manifest["artifacts"][role]["sha256"]
            and retained["provenance"] == manifest["artifacts"][role]["provenance"],
            f"contract artifact {role!r} does not match the manifest descriptor",
        )

    child = contract["child"]
    require(
        isinstance(child, dict) and set(child) == CONTRACT_CHILD_FIELDS,
        "contract child fields are invalid",
    )
    executable, executable_path = validate_contract_descriptor(
        child["executable"], "contract.child.executable", root,
    )
    manifest_runner, manifest_executable_path = validate_runner(
        manifest["runner"], lane, root, manifest["artifacts"]["recompiled"],
    )
    require(
        executable_path == manifest_executable_path
        and executable["bytes"] == manifest_runner["executable"]["length"]
        and executable["sha256"] == manifest_runner["executable"]["sha256"],
        "contract child executable does not match the manifest runner",
    )
    validate_local_directory(
        child["working_directory"], "contract.child.working_directory", root,
    )
    environment = child["environment"]
    require(isinstance(environment, list), "contract child environment must be an array")
    environment_names = [
        nonempty(entry.get("name") if isinstance(entry, dict) else None,
                 f"contract.child.environment[{index}].name")
        for index, entry in enumerate(environment)
    ]
    require(
        all(isinstance(entry, dict) and set(entry) == CONTRACT_ENVIRONMENT_FIELDS
            for entry in environment),
        "contract child environment fields are invalid",
    )
    require(environment_names == sorted(environment_names), "contract child environment is not sorted")
    require(len(environment_names) == len(set(environment_names)), "contract child environment repeats a name")
    contract_environment = {entry["name"]: entry["value"] for entry in environment}
    require(
        child["working_directory"] == manifest_runner["working_directory"]
        and child["argv"] == manifest_runner["argv"]
        and contract_environment == manifest_runner["env"],
        "contract child policy does not match the manifest runner",
    )

    claimed = validate_sha256(
        contract["contract_sha256"], "contract.contract_sha256",
    )
    unsigned = {
        key: value for key, value in contract.items()
        if key != "contract_sha256"
    }
    require(
        claimed == private_run_contract_sha256(unsigned),
        "private run contract SHA-256 drift",
    )


def write_private_run_contract(path: Path, contract: dict, root: Path) -> None:
    # A malformed contract must never become the final output, even briefly.
    validate_private_run_contract(contract, None, root)
    publish_complete_file_exclusive(
        path,
        serialize_json_document(contract),
        root,
        "--emit-private-run-contract",
    )


def descriptor(path: Path, provenance: str) -> dict:
    length, digest, _ = regular_file_measurement(path)
    return {
        "path": str(path),
        "length": length,
        "sha256": digest,
        "provenance": provenance,
        "git_identity": "excluded",
    }


def synthetic_manifest(directory: Path) -> tuple[Path, dict]:
    text = directory / "synthetic-text.bin"
    data = directory / "synthetic-data.bin"
    executable = directory / "synthetic-runner"
    text.write_bytes(bytes((index * 17 + 3) & 0xFF for index in range(4096)))
    data.write_bytes(bytes((index * 29 + 5) & 0xFF for index in range(256)))
    interpreter = Path(sys.executable).resolve(strict=True)
    require_native_executable(interpreter, "selftest Python interpreter")
    shutil.copyfile(interpreter, executable)
    executable.chmod(0o700)
    manifest = {
        "schema": MANIFEST_SCHEMA,
        "purpose": "extended_gbi",
        "intent": {
            "wire_family": "f3dex2_extended_gbi_v1",
            "report_scenario": "synthetic-private-admission-selftest",
            "recognition": "runtime_must_confirm_backend_known_pair",
            "extended_gbi_cases": sorted(EXTENDED_CASES),
            "program_evidence_lane": "no_program_fixture",
            "rom_class": "not_applicable",
        },
        "release_matrix": {
            "platform": "macos_arm64",
            "controllers": ["standard_controller"],
            "save": "no_cartridge_save",
            "renderers": ["rt64_lle_accuracy", "rt64_post_vi_capture"],
            "repeat_bar": 10,
        },
        "artifacts": {
            "microcode_text": descriptor(text, "user_owned_rom_derived"),
            "microcode_data": descriptor(data, "user_owned_rom_derived"),
            "microcode_text_raw_window": None,
            "microcode_data_raw_window": None,
            "rom": None,
            "recompiled": None,
        },
        "runner": {
            "executable": {
                "path": str(executable),
                "length": executable.stat().st_size,
                "sha256": sha256_file(executable),
                "git_identity": "excluded",
            },
            "working_directory": str(directory),
            "argv": ["--synthetic"],
            "env": {"FN64_SYNTHETIC_FIXED": "1"},
            "release_gate_cycle": 42,
            "execution_source": {"kind": "no_program"},
            "program_build_receipt": None,
        },
    }
    manifest_path = directory / "manifest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    return manifest_path, manifest


def expect_rejected(action, label: str) -> None:
    try:
        action()
    except AdmissionError:
        return
    raise AdmissionError(f"selftest: {label} was accepted")


def selftest(root: Path) -> None:
    canonical_program_receipt = {
        "schema": PROGRAM_BUILD_RECEIPT_SCHEMA,
        "child_executable": {
            "path": "/private/fn64/child",
            "bytes": 999,
            "sha256": "11" * 32,
        },
        "lane": {
            "kind": "typed_block",
            "pack": {
                "path": "/private/fn64/program.pack",
                "bytes": 789,
                "sha256": "22" * 32,
            },
            "expected_program_sha256": "33" * 32,
        },
        "expected_execution_source": {
            "kind": "typed_block_program",
            "program_sha256": "33" * 32,
            "dispatch_artifact_sha256": "22" * 32,
        },
    }
    require(
        program_build_receipt_sha256(canonical_program_receipt)
        == "3ce6e14e0a67c1837ca506e85815d20b6b9fe45f70b8a425ef2eeaf0ab6cd650",
        "selftest: program-build receipt canonical wire drifted",
    )
    canonical_fixture = {
        "schema": PRIVATE_RUN_CONTRACT_SCHEMA,
        "admission_manifest": {
            "path": "/private/manifest.json", "bytes": 123,
            "sha256": "00" * 32,
        },
        "readiness_report": {
            "path": "/private/readiness.json", "bytes": 456,
            "sha256": "11" * 32,
        },
        "program_build_receipt": {
            "path": "/private/program-build-receipt.json", "bytes": 654,
            "sha256": "12" * 32,
        },
        "purpose": "full_rom",
        "rom_class": "retail_cartridge",
        "report_scenario": "canonical-wire-fixture",
        "guest_cycle": 42,
        "repeat_count": 10,
        "input": {
            "role": "rom", "path": "/private/game.z64", "bytes": 67_108_864,
            "sha256": "22" * 32,
            "provenance": "user_owned_retail_cartridge_dump",
        },
        "admitted_artifacts": [
            {
                "role": "microcode_data", "path": "/private/ucode.data",
                "bytes": 128, "sha256": "33" * 32,
                "provenance": "user_owned_rom_derived",
            },
            {
                "role": "microcode_text", "path": "/private/ucode.text",
                "bytes": 4096, "sha256": "34" * 32,
                "provenance": "user_owned_rom_derived",
            },
            {
                "role": "recompiled", "path": "/private/game.a", "bytes": 789,
                "sha256": "44" * 32,
                "provenance": "user_generated_from_owned_rom",
            },
        ],
        "expected_execution_source": {
            "kind": "typed_block_program",
            "program_sha256": "55" * 32,
            "dispatch_artifact_sha256": "66" * 32,
        },
        "child": {
            "executable": {
                "path": "/private/game", "bytes": 999, "sha256": "77" * 32,
            },
            "working_directory": "/private/run",
            "argv": ["--headless", "value"],
            "environment": [
                {"name": "A_FIXED", "value": "1"},
                {"name": "Z_FIXED", "value": "two"},
            ],
        },
    }
    canonical_contract_sha256 = private_run_contract_sha256(canonical_fixture)
    require(
        canonical_contract_sha256
        == "e4ca4cf7a3a6beaf88515ffc04d235c74fabf63f8d99cec5f20cb359a13712b3",
        "selftest: private run-contract canonical wire drifted: "
        f"{canonical_contract_sha256}",
    )

    base = Path("/private/tmp") if Path("/private/tmp").is_dir() else Path(tempfile.gettempdir()).resolve()
    with tempfile.TemporaryDirectory(prefix="fn64-private-admission-", dir=base) as raw_directory:
        directory = Path(raw_directory)
        duplicate_json = directory / "duplicate.json"
        duplicate_json.write_text('{"env":{"A":"1","A":"2"}}\n', encoding="utf-8")
        expect_rejected(lambda: load_json(duplicate_json), "duplicate JSON field")
        manifest_path, manifest = synthetic_manifest(directory)
        readiness, _ = validate_manifest(manifest, manifest_path, root)
        validate_readiness(readiness)
        script = directory / "script-runner"
        script.write_bytes(b"#!/bin/sh\nexit 0\n")
        script.chmod(0o700)
        script_runner = json.loads(json.dumps(manifest))
        script_runner["runner"]["executable"] = {
            "path": str(script),
            "length": script.stat().st_size,
            "sha256": sha256_file(script),
            "git_identity": "excluded",
        }
        expect_rejected(
            lambda: validate_manifest(script_runner, manifest_path, root),
            "interpreter-mediated runner",
        )
        reserved_release_env = json.loads(json.dumps(manifest))
        reserved_release_env["runner"]["env"] = {"FN64_RELEASE_FUTURE": "1"}
        expect_rejected(
            lambda: validate_manifest(reserved_release_env, manifest_path, root),
            "future runner-owned release environment",
        )
        serialized = json.dumps(readiness, sort_keys=True)
        for private_value in (
            manifest["artifacts"]["microcode_text"]["path"],
            manifest["artifacts"]["microcode_text"]["sha256"],
            str(manifest["artifacts"]["microcode_text"]["length"]),
        ):
            require(private_value not in serialized, "selftest: readiness report leaked private identity")

        legacy_manifest = json.loads(json.dumps(manifest))
        legacy_manifest["schema"] = LEGACY_MANIFEST_SCHEMA
        legacy_manifest["artifacts"].pop("microcode_text_raw_window")
        legacy_manifest["artifacts"].pop("microcode_data_raw_window")
        legacy_readiness, _ = validate_manifest(
            legacy_manifest, manifest_path, root,
        )
        require(
            legacy_readiness["schema"] == LEGACY_READINESS_SCHEMA,
            "selftest: retained v6 manifest did not emit retained v5 readiness",
        )
        validate_readiness(legacy_readiness)
        legacy_new_wire = json.loads(json.dumps(legacy_manifest))
        legacy_new_wire["intent"]["wire_family"] = "f3dzex2"
        expect_rejected(
            lambda: validate_manifest(legacy_new_wire, manifest_path, root),
            "new F3DZEX2 wire family under retained v6 manifest schema",
        )
        legacy_new_wire_readiness = json.loads(json.dumps(legacy_readiness))
        legacy_new_wire_readiness["wire_family"] = "f3dzex2"
        expect_rejected(
            lambda: validate_readiness(legacy_new_wire_readiness),
            "new F3DZEX2 wire family under retained v5 readiness schema",
        )
        legacy_manifest_path = directory / "legacy-manifest.json"
        legacy_report_path = directory / "legacy-readiness.json"
        legacy_manifest_path.write_bytes(serialize_json_document(legacy_manifest))
        legacy_admission = subprocess.run(
            [
                sys.executable,
                str(Path(__file__).resolve()),
                "--manifest",
                str(legacy_manifest_path),
                "--report",
                str(legacy_report_path),
            ],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        )
        require(
            legacy_admission.returncode != 0 and not legacy_report_path.exists(),
            "selftest: retained v6 manifest compatibility was not read-only",
        )

        raw_text = directory / "synthetic-text-raw-window.bin"
        raw_data = directory / "synthetic-data-raw-window.bin"
        raw_text.write_bytes(
            bytes((index * 37 + 7) & 0xFF for index in range(0x18D0))
        )
        raw_data.write_bytes(
            bytes((index * 41 + 11) & 0xFF for index in range(0x0FC0))
        )
        characterization = json.loads(json.dumps(manifest))
        characterization["purpose"] = "f3dzex2_characterization"
        characterization["intent"]["wire_family"] = "f3dzex2"
        characterization["intent"]["report_scenario"] = (
            "synthetic-f3dzex2-characterization-selftest"
        )
        characterization["intent"]["extended_gbi_cases"] = []
        characterization["artifacts"]["microcode_text"] = None
        characterization["artifacts"]["microcode_data"] = None
        characterization["artifacts"]["microcode_text_raw_window"] = descriptor(
            raw_text, "user_owned_rom_derived",
        )
        characterization["artifacts"]["microcode_data_raw_window"] = descriptor(
            raw_data, "user_owned_rom_derived",
        )
        characterization_readiness, characterization_admitted = validate_manifest(
            characterization, manifest_path, root,
        )
        require(
            set(characterization_admitted)
            == {"microcode_text_raw_window", "microcode_data_raw_window"},
            "selftest: characterization admitted an ambiguous artifact set",
        )
        require(
            characterization_readiness["schema"] == READINESS_SCHEMA
            and characterization_readiness["purpose"]
            == "f3dzex2_characterization"
            and characterization_readiness["extended_gbi_fixture"]
            == "not_requested"
            and characterization_readiness["full_rom_inputs"]
            == "not_supplied",
            "selftest: characterization readiness policy drifted",
        )
        validate_readiness(characterization_readiness)
        characterization_serialized = json.dumps(
            characterization_readiness, sort_keys=True,
        )
        for role in ("microcode_text_raw_window", "microcode_data_raw_window"):
            private_descriptor = characterization["artifacts"][role]
            for private_value in (
                private_descriptor["path"],
                private_descriptor["sha256"],
                str(private_descriptor["length"]),
            ):
                require(
                    private_value not in characterization_serialized,
                    "selftest: characterization readiness leaked private identity",
                )

        legacy_characterization = json.loads(json.dumps(characterization))
        legacy_characterization["schema"] = LEGACY_MANIFEST_SCHEMA
        legacy_characterization["artifacts"].pop("microcode_text_raw_window")
        legacy_characterization["artifacts"].pop("microcode_data_raw_window")
        expect_rejected(
            lambda: validate_manifest(
                legacy_characterization, manifest_path, root,
            ),
            "F3DZEX2 characterization under retained v6 manifest schema",
        )
        legacy_characterization_readiness = json.loads(
            json.dumps(characterization_readiness)
        )
        legacy_characterization_readiness["schema"] = LEGACY_READINESS_SCHEMA
        expect_rejected(
            lambda: validate_readiness(legacy_characterization_readiness),
            "F3DZEX2 characterization under retained v5 readiness schema",
        )

        for role in ("microcode_text_raw_window", "microcode_data_raw_window"):
            for delta in (-1, 1):
                wrong_raw_length = json.loads(json.dumps(characterization))
                wrong_raw_length["artifacts"][role]["length"] += delta
                expect_rejected(
                    lambda value=wrong_raw_length: validate_manifest(
                        value, manifest_path, root,
                    ),
                    f"{role} off-by-one length {delta:+d}",
                )

        for missing_role in (
            "microcode_text_raw_window", "microcode_data_raw_window",
        ):
            missing_raw_role = json.loads(json.dumps(characterization))
            missing_raw_role["artifacts"][missing_role] = None
            expect_rejected(
                lambda value=missing_raw_role: validate_manifest(
                    value, manifest_path, root,
                ),
                f"characterization missing {missing_role}",
            )

        mixed_authority = json.loads(json.dumps(characterization))
        mixed_authority["artifacts"]["microcode_text"] = manifest["artifacts"][
            "microcode_text"
        ]
        expect_rejected(
            lambda: validate_manifest(mixed_authority, manifest_path, root),
            "characterization with logical and raw text authorities",
        )
        wrong_characterization_family = json.loads(json.dumps(characterization))
        wrong_characterization_family["intent"]["wire_family"] = "f3dex2"
        expect_rejected(
            lambda: validate_manifest(
                wrong_characterization_family, manifest_path, root,
            ),
            "characterization with neighboring wire family",
        )
        characterization_cases = json.loads(json.dumps(characterization))
        characterization_cases["intent"]["extended_gbi_cases"] = sorted(
            EXTENDED_CASES
        )
        expect_rejected(
            lambda: validate_manifest(
                characterization_cases, manifest_path, root,
            ),
            "characterization claiming Extended GBI cases",
        )
        characterization_program = json.loads(json.dumps(characterization))
        characterization_program["intent"]["program_evidence_lane"] = (
            "typed_block_program"
        )
        expect_rejected(
            lambda: validate_manifest(
                characterization_program, manifest_path, root,
            ),
            "characterization claiming an executable program lane",
        )
        characterization_reference = json.loads(json.dumps(characterization))
        characterization_reference["release_matrix"]["renderers"] = [
            "reference_lle_accuracy"
        ]
        expect_rejected(
            lambda: validate_manifest(
                characterization_reference, manifest_path, root,
            ),
            "characterization without RT64 post-VI coverage",
        )
        characterization_with_logical_readiness = json.loads(
            json.dumps(characterization_readiness)
        )
        characterization_with_logical_readiness[
            "artifact_roles_admitted"
        ].append("microcode_text")
        expect_rejected(
            lambda: validate_readiness(characterization_with_logical_readiness),
            "characterization readiness with mixed logical/raw authority",
        )

        text_path = Path(manifest["artifacts"]["microcode_text"]["path"])
        original = text_path.read_bytes()
        text_path.write_bytes(original[:-1] + bytes([original[-1] ^ 0xFF]))
        expect_rejected(lambda: validate_manifest(manifest, manifest_path, root), "hash drift")
        text_path.write_bytes(original)

        wrong_length = json.loads(json.dumps(manifest))
        wrong_length["artifacts"]["microcode_data"]["length"] += 1
        expect_rejected(lambda: validate_manifest(wrong_length, manifest_path, root), "length drift")

        shrunk = json.loads(json.dumps(manifest))
        shrunk["intent"]["extended_gbi_cases"].pop()
        expect_rejected(lambda: validate_manifest(shrunk, manifest_path, root), "shrunk Extended GBI case denominator")

        combined = json.loads(json.dumps(manifest))
        combined["purpose"] = "combined"
        expect_rejected(lambda: validate_manifest(combined, manifest_path, root), "combined admission without ROM/recompiled artifacts")

        rom = directory / "synthetic-rom.bin"
        recompiled = directory / "synthetic-recompiled.bin"
        rom.write_bytes(bytes(range(64)))
        recompiled.write_bytes(bytes(reversed(range(64))))
        full_rom = json.loads(json.dumps(manifest))
        full_rom["purpose"] = "full_rom"
        full_rom["intent"]["wire_family"] = "full_rom_mixed"
        full_rom["intent"]["extended_gbi_cases"] = []
        full_rom["intent"]["program_evidence_lane"] = "typed_block_program"
        full_rom["intent"]["rom_class"] = "retail_cartridge"
        recompiled_sha256 = sha256_file(recompiled)
        full_rom["runner"]["execution_source"] = {
            "kind": "typed_block_program",
            "program_sha256": "11" * 32,
            "dispatch_artifact_sha256": recompiled_sha256,
        }
        full_rom["release_matrix"]["renderers"] = ["reference_lle_accuracy"]
        full_rom["artifacts"]["rom"] = descriptor(
            rom, "user_owned_retail_cartridge_dump"
        )
        full_rom["artifacts"]["recompiled"] = descriptor(
            recompiled, "user_generated_from_owned_rom"
        )
        def write_program_receipt(
            path: Path, lane_value: dict, source: dict,
        ) -> dict:
            executable_value = full_rom["runner"]["executable"]
            unsigned_receipt = {
                "schema": PROGRAM_BUILD_RECEIPT_SCHEMA,
                "child_executable": {
                    "path": executable_value["path"],
                    "bytes": executable_value["length"],
                    "sha256": executable_value["sha256"],
                },
                "lane": lane_value,
                "expected_execution_source": source,
            }
            receipt = {
                **unsigned_receipt,
                "receipt_sha256": program_build_receipt_sha256(unsigned_receipt),
            }
            path.write_bytes(serialize_json_document(receipt))
            return {
                "path": str(path),
                "length": path.stat().st_size,
                "sha256": sha256_file(path),
                "git_identity": "excluded",
            }

        recompiled_identity = {
            "path": str(recompiled),
            "bytes": recompiled.stat().st_size,
            "sha256": recompiled_sha256,
        }
        full_rom["runner"]["program_build_receipt"] = write_program_receipt(
            directory / "typed-block-program-receipt.json",
            {
                "kind": "typed_block",
                "pack": recompiled_identity,
                "expected_program_sha256": "11" * 32,
            },
            full_rom["runner"]["execution_source"],
        )
        full_readiness, _ = validate_manifest(full_rom, manifest_path, root)
        validate_readiness(full_readiness)

        relabelled_homebrew = json.loads(json.dumps(full_rom))
        relabelled_homebrew["intent"]["rom_class"] = "public_homebrew"
        expect_rejected(
            lambda: validate_manifest(relabelled_homebrew, manifest_path, root),
            "public-homebrew relabel over retail-cartridge provenance",
        )
        ambiguous_rom = json.loads(json.dumps(full_rom))
        ambiguous_rom["artifacts"]["rom"]["provenance"] = "user_owned_rom"
        expect_rejected(
            lambda: validate_manifest(ambiguous_rom, manifest_path, root),
            "ambiguous legacy ROM provenance",
        )
        public_homebrew = json.loads(json.dumps(full_rom))
        public_homebrew["intent"]["rom_class"] = "public_homebrew"
        public_homebrew["artifacts"]["rom"]["provenance"] = (
            "publicly_distributed_homebrew_rom"
        )
        public_homebrew_readiness, _ = validate_manifest(
            public_homebrew, manifest_path, root
        )
        validate_readiness(public_homebrew_readiness)

        observed_function = json.loads(json.dumps(full_rom))
        observed_function["intent"]["program_evidence_lane"] = "typed_observed_function"
        observed_function["runner"]["execution_source"] = {
            "kind": "typed_observed_function_program",
            "artifact_sha256": recompiled_sha256,
        }
        observed_function["runner"]["program_build_receipt"] = write_program_receipt(
            directory / "typed-observed-program-receipt.json",
            {
                "kind": "typed_observed_function",
                "identity_wire": recompiled_identity,
            },
            observed_function["runner"]["execution_source"],
        )
        observed_function_readiness, _ = validate_manifest(
            observed_function, manifest_path, root
        )
        validate_readiness(observed_function_readiness)

        native_identity = {
            "path": str(recompiled),
            "bytes": recompiled.stat().st_size,
            "sha256": recompiled_sha256,
        }
        native_source = {
            "kind": "native_archive",
            "artifact_sha256": native_archive_source_sha256([
                ("0-generated", native_identity, recompiled),
            ]),
        }
        stale_native_identity = dict(native_identity)
        stale_native_identity["sha256"] = "00" * 32
        expect_rejected(
            lambda: native_archive_source_sha256([
                ("0-generated", stale_native_identity, recompiled),
            ]),
            "native archive aggregate with stale per-file digest",
        )
        native = json.loads(json.dumps(full_rom))
        native["intent"]["program_evidence_lane"] = "identified_native_archive"
        native["runner"]["execution_source"] = native_source
        native["runner"]["program_build_receipt"] = write_program_receipt(
            directory / "native-program-receipt.json",
            {
                "kind": "native_archives",
                "archives": [{"label": "0-generated", "file": native_identity}],
            },
            native_source,
        )
        native_readiness, _ = validate_manifest(native, manifest_path, root)
        validate_readiness(native_readiness)

        typed_function = json.loads(json.dumps(full_rom))
        typed_function["intent"]["program_evidence_lane"] = "typed_function"
        expect_rejected(
            lambda: validate_manifest(typed_function, manifest_path, root),
            "typed whole-function full-ROM lane",
        )

        unidentified_native = json.loads(json.dumps(full_rom))
        unidentified_native["intent"]["program_evidence_lane"] = "unidentified_native"
        expect_rejected(
            lambda: validate_manifest(unidentified_native, manifest_path, root),
            "unidentified native full-ROM lane",
        )

        no_program_full_rom = json.loads(json.dumps(full_rom))
        no_program_full_rom["intent"]["program_evidence_lane"] = "no_program_fixture"
        expect_rejected(
            lambda: validate_manifest(no_program_full_rom, manifest_path, root),
            "no-program full-ROM lane",
        )

        directory_descriptor = json.loads(json.dumps(manifest))
        directory_descriptor["artifacts"]["microcode_data"]["path"] = str(directory)
        expect_rejected(lambda: validate_manifest(directory_descriptor, manifest_path, root), "special/non-regular file")

        tracked = json.loads(json.dumps(manifest))
        readme = root / "README.md"
        tracked["artifacts"]["microcode_data"] = {
            "path": str(readme), "length": readme.stat().st_size,
            "sha256": sha256_file(readme), "provenance": "user_owned_rom_derived",
            "git_identity": "excluded",
        }
        expect_rejected(lambda: validate_manifest(tracked, manifest_path, root), "tracked repository input")

        case_variant_readme = root / "readme.md"
        try:
            case_variant_supported = (
                case_variant_readme != readme
                and case_variant_readme.exists()
                and os.path.samefile(case_variant_readme, readme)
            )
        except OSError:
            case_variant_supported = False
        if case_variant_supported:
            case_variant_tracked = json.loads(json.dumps(manifest))
            case_variant_tracked["artifacts"]["microcode_data"] = {
                "path": str(case_variant_readme),
                "length": readme.stat().st_size,
                "sha256": sha256_file(readme),
                "provenance": "user_owned_rom_derived",
                "git_identity": "excluded",
            }
            require(
                filesystem_relative_to(case_variant_readme, root)
                == Path("README.md"),
                "selftest: filesystem spelling was not recovered for a case variant",
            )
            expect_rejected(
                lambda: validate_manifest(
                    case_variant_tracked, manifest_path, root,
                ),
                "case-variant tracked repository input",
            )

        link = directory / "text-link.bin"
        try:
            link.symlink_to(text_path)
        except OSError:
            pass
        else:
            symlinked = json.loads(json.dumps(manifest))
            symlinked["artifacts"]["microcode_text"]["path"] = str(link)
            expect_rejected(lambda: validate_manifest(symlinked, manifest_path, root), "symlink input")
            manifest_link = directory / "manifest-link.json"
            manifest_link.symlink_to(manifest_path)
            expect_rejected(lambda: validate_manifest(manifest, manifest_link, root), "symlink manifest")
            working_link = directory / "working-link"
            working_link.symlink_to(directory, target_is_directory=True)
            symlinked_working_directory = json.loads(json.dumps(manifest))
            symlinked_working_directory["runner"]["working_directory"] = str(
                working_link
            )
            expect_rejected(
                lambda: validate_manifest(
                    symlinked_working_directory, manifest_path, root,
                ),
                "symlink runner working directory",
            )

        fifo = directory / "synthetic.fifo"
        try:
            os.mkfifo(fifo)
        except (AttributeError, OSError):
            pass
        else:
            fifo_manifest = json.loads(json.dumps(manifest))
            fifo_manifest["artifacts"]["microcode_data"]["path"] = str(fifo)
            expect_rejected(lambda: validate_manifest(fifo_manifest, manifest_path, root), "FIFO input")

        report_path = directory / "readiness.json"
        write_report(report_path, readiness, root)
        validate_readiness(load_json(report_path))
        expect_rejected(lambda: write_report(report_path, readiness, root), "readiness overwrite")

        for reserved_name in (
            "ROM",
            "FN64_RELEASE_GATE_CYCLE",
            "FN64_RELEASE_REPORT",
            "FN64_RELEASE_RUN_EVENT_SHA256",
            "OOT_RELEASE_DISCOVER_QUIESCENT_AFTER",
        ):
            reserved_environment = json.loads(json.dumps(full_rom))
            reserved_environment["runner"]["env"][reserved_name] = "/tmp/forged"
            expect_rejected(
                lambda value=reserved_environment: validate_manifest(
                    value, manifest_path, root,
                ),
                f"reserved runner environment {reserved_name}",
            )

        lowercase_environment = json.loads(json.dumps(full_rom))
        lowercase_environment["runner"]["env"]["lowercase"] = "forbidden"
        expect_rejected(
            lambda: validate_manifest(lowercase_environment, manifest_path, root),
            "lowercase runner environment name",
        )

        forbidden_environment_names = sorted(FORBIDDEN_RUNNER_ENV) + [
            f"{prefix}FN64_INJECT"
            for prefix in FORBIDDEN_RUNNER_ENV_PREFIXES
        ]
        for forbidden_name in forbidden_environment_names:
            forbidden_environment = json.loads(json.dumps(full_rom))
            forbidden_environment["runner"]["env"][forbidden_name] = "/tmp/inject"
            expect_rejected(
                lambda value=forbidden_environment: validate_manifest(
                    value, manifest_path, root,
                ),
                f"code-injecting runner environment {forbidden_name}",
            )

        wrong_source = json.loads(json.dumps(full_rom))
        wrong_source["runner"]["execution_source"] = {
            "kind": "native_archive",
            "artifact_sha256": "44" * 32,
        }
        expect_rejected(
            lambda: validate_manifest(wrong_source, manifest_path, root),
            "execution-source lane mismatch",
        )

        invalid_cycle = json.loads(json.dumps(full_rom))
        invalid_cycle["runner"]["release_gate_cycle"] = -1
        expect_rejected(
            lambda: validate_manifest(invalid_cycle, manifest_path, root),
            "negative release gate cycle",
        )

        extended_readiness_path = directory / "extended-readiness.json"
        write_report(extended_readiness_path, readiness, root)
        expect_rejected(
            lambda: build_private_run_contract(
                manifest, manifest_path, extended_readiness_path,
                validate_manifest(manifest, manifest_path, root)[1],
            ),
            "private contract for no-program Extended GBI fixture",
        )

        full_manifest_path = directory / "full-manifest.json"
        full_manifest_path.write_text(
            json.dumps(full_rom, indent=2) + "\n", encoding="utf-8",
        )
        full_readiness, full_admitted = validate_manifest(
            full_rom, full_manifest_path, root,
        )
        preflight_readiness_path = directory / "preflight-readiness.json"
        preflight_readiness_payload = serialize_json_document(full_readiness)
        preflight_contract = build_private_run_contract(
            full_rom,
            full_manifest_path,
            preflight_readiness_path,
            full_admitted,
            preflight_readiness_payload,
        )
        validate_private_run_contract(
            preflight_contract,
            None,
            root,
            (
                preflight_readiness_path,
                full_readiness,
                preflight_readiness_payload,
            ),
        )
        require(
            not preflight_readiness_path.exists(),
            "selftest: in-memory contract preflight published its readiness input",
        )
        full_readiness_path = directory / "full-readiness.json"
        write_report(full_readiness_path, full_readiness, root)
        contract = build_private_run_contract(
            full_rom, full_manifest_path, full_readiness_path, full_admitted,
        )
        contract_path = directory / "private-run-contract.json"
        write_private_run_contract(contract_path, contract, root)
        validate_private_run_contract(load_json(contract_path), contract_path, root)
        expect_rejected(
            lambda: write_private_run_contract(contract_path, contract, root),
            "private run-contract overwrite",
        )

        invalid_contract = json.loads(json.dumps(contract))
        invalid_contract["repeat_count"] = 9
        invalid_contract_path = directory / "invalid-private-run-contract.json"
        expect_rejected(
            lambda: write_private_run_contract(
                invalid_contract_path, invalid_contract, root,
            ),
            "invalid private run-contract publication",
        )
        require(
            not invalid_contract_path.exists(),
            "selftest: invalid private run contract was left at its final path",
        )
        require(
            not any(
                entry.name.startswith(".invalid-private-run-contract.json.fn64-stage-")
                for entry in directory.iterdir()
            ),
            "selftest: rejected private run contract left a staging file",
        )

        def resign(value: dict) -> dict:
            changed = json.loads(json.dumps(value))
            unsigned = {
                key: item for key, item in changed.items()
                if key != "contract_sha256"
            }
            changed["contract_sha256"] = private_run_contract_sha256(unsigned)
            return changed

        wrong_repeat = json.loads(json.dumps(contract))
        wrong_repeat["repeat_count"] = 9
        expect_rejected(
            lambda: validate_private_run_contract(
                resign(wrong_repeat), contract_path, root,
            ),
            "private run contract repeat-count drift",
        )

        wrong_contract_source = json.loads(json.dumps(contract))
        wrong_contract_source["expected_execution_source"] = {
            "kind": "native_archive",
            "artifact_sha256": "44" * 32,
        }
        expect_rejected(
            lambda: validate_private_run_contract(
                resign(wrong_contract_source), contract_path, root,
            ),
            "private run contract execution-source drift",
        )

        wrong_rom_class = json.loads(json.dumps(contract))
        wrong_rom_class["rom_class"] = "public_homebrew"
        expect_rejected(
            lambda: validate_private_run_contract(
                resign(wrong_rom_class), contract_path, root,
            ),
            "private run contract ROM-class drift",
        )

        wrong_rom_provenance = json.loads(json.dumps(contract))
        wrong_rom_provenance["input"]["provenance"] = (
            "publicly_distributed_homebrew_rom"
        )
        expect_rejected(
            lambda: validate_private_run_contract(
                resign(wrong_rom_provenance), contract_path, root,
            ),
            "private run contract ROM provenance/class mismatch",
        )

        reordered_artifacts = json.loads(json.dumps(contract))
        reordered_artifacts["admitted_artifacts"].reverse()
        expect_rejected(
            lambda: validate_private_run_contract(
                resign(reordered_artifacts), contract_path, root,
            ),
            "private run contract artifact-order drift",
        )

        duplicate_environment = json.loads(json.dumps(contract))
        duplicate_environment["child"]["environment"].append(
            dict(duplicate_environment["child"]["environment"][0])
        )
        expect_rejected(
            lambda: validate_private_run_contract(
                resign(duplicate_environment), contract_path, root,
            ),
            "private run contract duplicate environment name",
        )

        wrong_manifest_identity = json.loads(json.dumps(contract))
        wrong_manifest_identity["admission_manifest"]["sha256"] = "55" * 32
        expect_rejected(
            lambda: validate_private_run_contract(
                resign(wrong_manifest_identity), contract_path, root,
            ),
            "private run contract manifest identity drift",
        )

        wrong_contract_hash = json.loads(json.dumps(contract))
        wrong_contract_hash["contract_sha256"] = "66" * 32
        expect_rejected(
            lambda: validate_private_run_contract(
                wrong_contract_hash, contract_path, root,
            ),
            "private run contract digest drift",
        )


def main() -> int:
    parser = argparse.ArgumentParser()
    actions = parser.add_mutually_exclusive_group(required=True)
    actions.add_argument("--check", action="store_true", help="run the synthetic policy selftest")
    actions.add_argument("--selftest", action="store_true")
    actions.add_argument("--manifest", type=Path)
    actions.add_argument("--verify-readiness", type=Path)
    actions.add_argument("--verify-private-run-contract", type=Path)
    parser.add_argument("--report", type=Path)
    parser.add_argument("--emit-private-run-contract", type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    try:
        if args.check or args.selftest:
            require(
                args.report is None and args.emit_private_run_contract is None,
                "--report/--emit-private-run-contract require --manifest",
            )
            selftest(root)
            print("private-input-admission: selftest passed (synthetic non-game bytes only)")
            return 0
        if args.verify_readiness:
            require(
                args.report is None and args.emit_private_run_contract is None,
                "--report/--emit-private-run-contract cannot accompany --verify-readiness",
            )
            validate_readiness(load_json(args.verify_readiness))
            print(f"private-input-admission: valid content-free readiness report {args.verify_readiness}")
            return 0
        if args.verify_private_run_contract:
            require(
                args.report is None and args.emit_private_run_contract is None,
                "--report/--emit-private-run-contract cannot accompany "
                "--verify-private-run-contract",
            )
            validate_private_run_contract(
                load_json(args.verify_private_run_contract),
                args.verify_private_run_contract,
                root,
            )
            print(
                "private-input-admission: valid content-bearing private run "
                f"contract {args.verify_private_run_contract}"
            )
            return 0
        require(args.manifest is not None and args.report is not None, "--manifest requires --report")
        require(args.manifest.is_absolute(), "--manifest must be absolute")
        validate_local_regular_file(str(args.manifest), "manifest", root)
        manifest = load_json(args.manifest)
        require(
            manifest.get("schema") == MANIFEST_SCHEMA,
            f"new --manifest admission requires schema {MANIFEST_SCHEMA!r}; "
            f"retained {LEGACY_MANIFEST_SCHEMA!r} is read-only compatibility for contract verification",
        )
        readiness, admitted = validate_manifest(manifest, args.manifest, root)
        if args.emit_private_run_contract is not None:
            require(
                manifest["purpose"] in {"full_rom", "combined"},
                "--emit-private-run-contract requires purpose full_rom or combined",
            )
            require(
                args.emit_private_run_contract != args.report,
                "--emit-private-run-contract and --report must be distinct paths",
            )
            validate_output_path(
                args.emit_private_run_contract, root, "--emit-private-run-contract",
            )
            readiness_payload = serialize_json_document(readiness)
            contract = build_private_run_contract(
                manifest,
                args.manifest,
                args.report,
                admitted,
                readiness_payload,
            )
            validate_private_run_contract(
                contract,
                None,
                root,
                (args.report, readiness, readiness_payload),
            )
        write_report(args.report, readiness, root)
        if args.emit_private_run_contract is not None:
            write_private_run_contract(
                args.emit_private_run_contract, contract, root,
            )
            print(
                "private-input-admission: ready; content-free report written to "
                f"{args.report}; content-bearing private run contract written to "
                f"{args.emit_private_run_contract}"
            )
        else:
            print(f"private-input-admission: ready; content-free report written to {args.report}")
        return 0
    except (AdmissionError, OSError) as error:
        print(f"private-input-admission: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
