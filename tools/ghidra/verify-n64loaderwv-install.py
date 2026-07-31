#!/usr/bin/env python3
"""Verify that an isolated Ghidra profile contains only the approved loader."""

from __future__ import annotations

from io import BytesIO
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import stat
import sys
from typing import NoReturn
import zipfile


LOADER_CLASS = "n64loaderwv/N64LoaderWVLoader.class"
SCHEMA = "fn64.n64loaderwv-install-verification"
MAX_ARCHIVE_BYTES = 128 * 1024 * 1024
MAX_UNCOMPRESSED_BYTES = 256 * 1024 * 1024
MAX_SCAN_FILES = 100_000
MAX_JAR_BYTES = 256 * 1024 * 1024
SAFE_ROOT = re.compile(r"[A-Za-z0-9._+\-]{1,128}\Z")


def fail(message: str) -> NoReturn:
    raise SystemExit(f"n64loaderwv install: {message}")


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def absolute_regular(path_value: str, label: str, limit: int) -> tuple[Path, bytes]:
    path = Path(path_value)
    if not path.is_absolute():
        fail(f"{label} must be absolute")
    if path.is_symlink() or not path.is_file():
        fail(f"{label} must be a regular non-symlink file")
    try:
        size = path.stat().st_size
        if size <= 0 or size > limit:
            fail(f"{label} size is outside 1..={limit} bytes")
        data = path.read_bytes()
        if len(data) != size or path.is_symlink() or path.stat().st_size != size:
            fail(f"{label} changed while reading")
        return path.resolve(strict=True), data
    except OSError as error:
        fail(f"could not read {label}: {error}")


def absolute_directory(path_value: str, label: str) -> Path:
    path = Path(path_value)
    if not path.is_absolute():
        fail(f"{label} must be absolute")
    if path.is_symlink() or not path.is_dir():
        fail(f"{label} must be a non-symlink directory")
    try:
        return path.resolve(strict=True)
    except OSError as error:
        fail(f"could not resolve {label}: {error}")


def safe_archive_name(info: zipfile.ZipInfo) -> tuple[PurePosixPath, bool]:
    name = info.filename
    if not name or "\\" in name or "\x00" in name or name.startswith("/"):
        fail("extension ZIP contains an unsafe entry name")
    directory = info.is_dir()
    normalized = name[:-1] if directory and name.endswith("/") else name
    parts = PurePosixPath(normalized).parts
    if not parts or any(part in ("", ".", "..") for part in parts):
        fail("extension ZIP contains an unsafe entry name")
    if "/".join(parts) != normalized:
        fail("extension ZIP contains a non-canonical entry name")
    mode = (info.external_attr >> 16) & 0xFFFF
    kind = stat.S_IFMT(mode)
    if kind and kind not in (stat.S_IFREG, stat.S_IFDIR):
        fail("extension ZIP contains a non-file entry")
    if kind == stat.S_IFDIR and not directory:
        fail("extension ZIP directory entry lacks a trailing slash")
    if kind == stat.S_IFREG and directory:
        fail("extension ZIP file entry has a trailing slash")
    return PurePosixPath(*parts), directory


def archive_tree(data: bytes) -> tuple[str, dict[PurePosixPath, bytes], set[PurePosixPath]]:
    try:
        with zipfile.ZipFile(BytesIO(data)) as archive:
            seen: set[PurePosixPath] = set()
            files: dict[PurePosixPath, bytes] = {}
            directories: set[PurePosixPath] = {PurePosixPath(".")}
            roots: set[str] = set()
            total = 0
            for info in archive.infolist():
                path, is_directory = safe_archive_name(info)
                if path in seen:
                    fail("extension ZIP contains duplicate entries")
                seen.add(path)
                roots.add(path.parts[0])
                relative = PurePosixPath(*path.parts[1:])
                if relative == PurePosixPath("."):
                    if not is_directory:
                        fail("extension ZIP root must be a directory")
                    continue
                for parent in relative.parents:
                    directories.add(parent)
                if is_directory:
                    directories.add(relative)
                    continue
                total += info.file_size
                if info.file_size > MAX_UNCOMPRESSED_BYTES or total > MAX_UNCOMPRESSED_BYTES:
                    fail("extension ZIP uncompressed size is too large")
                payload = archive.read(info)
                if len(payload) != info.file_size:
                    fail("extension ZIP entry changed while reading")
                files[relative] = payload
    except (OSError, RuntimeError, zipfile.BadZipFile, zipfile.LargeZipFile) as error:
        fail(f"could not inspect extension ZIP: {error}")
    if len(roots) != 1:
        fail("extension ZIP must contain exactly one top-level directory")
    root = next(iter(roots))
    if SAFE_ROOT.fullmatch(root) is None:
        fail("extension ZIP has an invalid top-level directory")
    if not files:
        fail("extension ZIP contains no files")
    return root, files, directories


def disk_tree(root: Path) -> tuple[dict[PurePosixPath, Path], set[PurePosixPath]]:
    files: dict[PurePosixPath, Path] = {}
    directories: set[PurePosixPath] = {PurePosixPath(".")}
    stack = [(root, PurePosixPath("."))]
    while stack:
        directory, relative = stack.pop()
        try:
            entries = list(os.scandir(directory))
        except OSError as error:
            fail(f"could not inspect extracted extension: {error}")
        for entry in entries:
            child_relative = (
                PurePosixPath(entry.name)
                if relative == PurePosixPath(".")
                else relative / entry.name
            )
            try:
                if entry.is_symlink():
                    fail("extracted extension contains a symlink")
                if entry.is_dir(follow_symlinks=False):
                    directories.add(child_relative)
                    stack.append((Path(entry.path), child_relative))
                elif entry.is_file(follow_symlinks=False):
                    files[child_relative] = Path(entry.path)
                else:
                    fail("extracted extension contains a special file")
            except OSError as error:
                fail(f"could not inspect extracted extension: {error}")
    return files, directories


def verify_extracted_tree(
    extension: Path,
    expected_files: dict[PurePosixPath, bytes],
    expected_directories: set[PurePosixPath],
) -> dict[PurePosixPath, Path]:
    actual_files, actual_directories = disk_tree(extension)
    if set(actual_files) != set(expected_files) or actual_directories != expected_directories:
        fail("extracted extension tree does not exactly match the approved ZIP")
    for relative, expected in expected_files.items():
        path = actual_files[relative]
        try:
            actual = path.read_bytes()
        except OSError as error:
            fail(f"could not read extracted extension file: {error}")
        if path.is_symlink() or len(actual) != len(expected) or sha256(actual) != sha256(expected):
            fail("extracted extension bytes do not match the approved ZIP")
    return actual_files


def loader_identity(
    files: dict[PurePosixPath, bytes], actual_files: dict[PurePosixPath, Path]
) -> tuple[Path, bytes, bytes]:
    matches: list[tuple[PurePosixPath, bytes, bytes]] = []
    for relative, jar_data in files.items():
        if relative.suffix.lower() != ".jar":
            continue
        try:
            with zipfile.ZipFile(BytesIO(jar_data)) as jar:
                class_entries = [info for info in jar.infolist() if info.filename == LOADER_CLASS]
                if len(class_entries) > 1:
                    fail("approved JAR contains duplicate loader classes")
                if class_entries:
                    class_data = jar.read(class_entries[0])
                    matches.append((relative, jar_data, class_data))
        except (RuntimeError, zipfile.BadZipFile, zipfile.LargeZipFile) as error:
            fail(f"approved extension contains an invalid JAR: {error}")
    if len(matches) != 1:
        fail("approved extension must contain exactly one loader class in one JAR")
    relative, jar_data, class_data = matches[0]
    if not class_data:
        fail("approved loader class is empty")
    return actual_files[relative], jar_data, class_data


def walk_scan(root: Path) -> list[Path]:
    files: list[Path] = []
    stack = [root]
    while stack:
        directory = stack.pop()
        try:
            entries = list(os.scandir(directory))
        except OSError as error:
            fail(f"could not scan loader classpath: {error}")
        for entry in entries:
            try:
                if entry.is_symlink():
                    fail("loader classpath contains a symlink")
                if entry.is_dir(follow_symlinks=False):
                    stack.append(Path(entry.path))
                elif entry.is_file(follow_symlinks=False):
                    files.append(Path(entry.path))
                    if len(files) > MAX_SCAN_FILES:
                        fail("loader classpath file count is too large")
                else:
                    fail("loader classpath contains a special file")
            except OSError as error:
                fail(f"could not scan loader classpath: {error}")
    return files


def scan_classpath(roots: tuple[Path, Path], approved_jar: Path) -> None:
    approved_seen = 0
    for root in roots:
        for path in walk_scan(root):
            relative_parts = path.relative_to(root).parts
            if len(relative_parts) >= 2 and relative_parts[-2:] == (
                "n64loaderwv",
                "N64LoaderWVLoader.class",
            ):
                fail("loader classpath contains a competing loose loader class")
            if path.suffix.lower() != ".jar":
                continue
            _, jar_data = absolute_regular(os.fspath(path), "classpath JAR", MAX_JAR_BYTES)
            try:
                with zipfile.ZipFile(BytesIO(jar_data)) as jar:
                    count = sum(1 for info in jar.infolist() if info.filename == LOADER_CLASS)
            except (RuntimeError, zipfile.BadZipFile, zipfile.LargeZipFile) as error:
                fail(f"loader classpath contains an invalid JAR: {error}")
            if count == 0:
                continue
            if count != 1 or path != approved_jar:
                fail("loader classpath contains a competing loader class")
            approved_seen += 1
    if approved_seen != 1:
        fail("approved loader JAR was not found exactly once in the profile classpath")


def main(arguments: list[str]) -> None:
    if len(arguments) != 5:
        fail(
            "usage: verify-n64loaderwv-install.py APPROVED_ZIP "
            "EXTRACTED_EXTENSION_DIR GHIDRA_INSTALL_DIR PROFILE_SETTINGS_ROOT"
        )
    _, zip_data = absolute_regular(arguments[1], "approved ZIP", MAX_ARCHIVE_BYTES)
    extension = absolute_directory(arguments[2], "extracted extension directory")
    ghidra = absolute_directory(arguments[3], "Ghidra install directory")
    profile = absolute_directory(arguments[4], "profile settings root")
    try:
        extension.relative_to(profile)
    except ValueError:
        fail("extracted extension directory must be inside the profile settings root")

    extension_root, expected_files, expected_directories = archive_tree(zip_data)
    if extension.name != extension_root:
        fail("extracted extension directory does not match the ZIP root")
    actual_files = verify_extracted_tree(
        extension, expected_files, expected_directories
    )
    approved_jar, jar_data, class_data = loader_identity(expected_files, actual_files)
    scan_classpath((ghidra, profile), approved_jar)
    result = {
        "extension_root": extension_root,
        "loader_class": {"byte_length": len(class_data), "sha256": sha256(class_data)},
        "loader_jar": {"byte_length": len(jar_data), "sha256": sha256(jar_data)},
        "schema": SCHEMA,
        "schema_version": 1,
    }
    print(json.dumps(result, sort_keys=True, separators=(",", ":")))


if __name__ == "__main__":
    main(sys.argv)
