#!/usr/bin/env python3
"""Create or verify a path-free, content-hashed Ghidra distribution inventory."""

from __future__ import annotations

import hashlib
import json
import os
import stat
import sys
import tempfile
from pathlib import Path

SCHEMA = "fn64.ghidra-distribution-manifest"
SCHEMA_VERSION = 1
MAX_FILES = 65_536
MAX_MANIFEST_BYTES = 16 * 1024 * 1024
BUFFER_BYTES = 1024 * 1024


def fail(message: str) -> "NoReturn":
    raise SystemExit(f"ghidra-distribution-manifest: {message}")


def canonical_directory(value: str, label: str) -> Path:
    path = Path(value)
    if not path.is_absolute():
        fail(f"{label} must be absolute")
    try:
        resolved = path.resolve(strict=True)
    except OSError as error:
        fail(f"cannot resolve {label}: {error}")
    if resolved != path or not resolved.is_dir():
        fail(f"{label} must be a canonical directory")
    return resolved


def hash_regular_file(path: Path) -> tuple[int, str]:
    try:
        before = path.lstat()
    except OSError as error:
        fail(f"cannot inspect {path}: {error}")
    if not stat.S_ISREG(before.st_mode):
        fail(f"distribution entry is not a regular non-symlink file: {path}")
    flags = os.O_RDONLY
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    try:
        descriptor = os.open(path, flags)
    except OSError as error:
        fail(f"cannot open {path}: {error}")
    digest = hashlib.sha256()
    measured = 0
    try:
        opened = os.fstat(descriptor)
        if not stat.S_ISREG(opened.st_mode) or (opened.st_dev, opened.st_ino) != (
            before.st_dev,
            before.st_ino,
        ):
            fail(f"distribution entry changed while opening: {path}")
        while True:
            block = os.read(descriptor, BUFFER_BYTES)
            if not block:
                break
            measured += len(block)
            digest.update(block)
        after = os.fstat(descriptor)
    finally:
        os.close(descriptor)
    stable_fields = ("st_dev", "st_ino", "st_size", "st_mtime_ns", "st_ctime_ns")
    if measured != before.st_size or any(
        getattr(before, field) != getattr(after, field) for field in stable_fields
    ):
        fail(f"distribution entry changed while hashing: {path}")
    return measured, digest.hexdigest()


def read_bounded_regular_file(path: Path, label: str) -> bytes:
    try:
        before = path.lstat()
    except OSError as error:
        fail(f"cannot inspect {label}: {error}")
    if not stat.S_ISREG(before.st_mode) or before.st_size > MAX_MANIFEST_BYTES:
        fail(f"{label} must be a bounded regular non-symlink file")
    flags = os.O_RDONLY
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    try:
        descriptor = os.open(path, flags)
    except OSError as error:
        fail(f"cannot open {label}: {error}")
    try:
        opened = os.fstat(descriptor)
        if not stat.S_ISREG(opened.st_mode) or (opened.st_dev, opened.st_ino) != (
            before.st_dev,
            before.st_ino,
        ):
            fail(f"{label} changed while opening")
        chunks: list[bytes] = []
        measured = 0
        while True:
            block = os.read(descriptor, min(BUFFER_BYTES, MAX_MANIFEST_BYTES + 1 - measured))
            if not block:
                break
            chunks.append(block)
            measured += len(block)
            if measured > MAX_MANIFEST_BYTES:
                fail(f"{label} exceeds {MAX_MANIFEST_BYTES} bytes")
        after = os.fstat(descriptor)
    finally:
        os.close(descriptor)
    stable_fields = ("st_dev", "st_ino", "st_size", "st_mtime_ns", "st_ctime_ns")
    if measured != before.st_size or any(
        getattr(before, field) != getattr(after, field) for field in stable_fields
    ):
        fail(f"{label} changed while reading")
    return b"".join(chunks)


def inventory(root: Path) -> bytes:
    entries: list[dict[str, object]] = []

    def traversal_failed(error: OSError) -> None:
        fail(f"cannot traverse distribution: {error}")

    for current, directory_names, file_names in os.walk(
        root, topdown=True, onerror=traversal_failed, followlinks=False
    ):
        directory_names.sort()
        file_names.sort()
        current_path = Path(current)
        for name in directory_names:
            path = current_path / name
            try:
                mode = path.lstat().st_mode
            except OSError as error:
                fail(f"cannot inspect {path}: {error}")
            if not stat.S_ISDIR(mode):
                fail(f"distribution directory entry is a symlink or special file: {path}")
        for name in file_names:
            path = current_path / name
            length, digest = hash_regular_file(path)
            relative = path.relative_to(root).as_posix()
            entries.append(
                {"path": relative, "byte_length": length, "sha256": digest}
            )
            if len(entries) > MAX_FILES:
                fail(f"distribution contains more than {MAX_FILES} files")
    entries.sort(key=lambda entry: entry["path"])
    value = {
        "schema": SCHEMA,
        "schema_version": SCHEMA_VERSION,
        "files": entries,
    }
    encoded = (
        json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n"
    ).encode("utf-8")
    if len(encoded) > MAX_MANIFEST_BYTES:
        fail(f"manifest exceeds {MAX_MANIFEST_BYTES} bytes")
    return encoded


def require_new_output(value: str) -> Path:
    path = Path(value)
    if not path.is_absolute():
        fail("OUTPUT must be absolute")
    try:
        parent = path.parent.resolve(strict=True)
    except OSError as error:
        fail(f"cannot resolve OUTPUT parent: {error}")
    if parent != path.parent:
        fail("OUTPUT parent must be canonical")
    try:
        path.lstat()
    except FileNotFoundError:
        return path
    except OSError as error:
        fail(f"cannot inspect OUTPUT: {error}")
    fail("refusing to overwrite OUTPUT")


def cache_inventory(encoded: bytes, cache_directory: Path) -> Path:
    digest = hashlib.sha256(encoded).hexdigest()
    cached = cache_directory / f"{digest}.json"
    try:
        cached.lstat()
    except FileNotFoundError:
        pass
    except OSError as error:
        fail(f"cannot inspect cached manifest: {error}")
    else:
        existing = read_bounded_regular_file(cached, "cached manifest")
        if existing != encoded:
            fail("content-addressed cache collision")
        return cached

    descriptor, temporary_name = tempfile.mkstemp(
        prefix=".ghidra-distribution.", dir=cache_directory
    )
    temporary = Path(temporary_name)
    try:
        os.fchmod(descriptor, 0o600)
        with os.fdopen(descriptor, "wb", closefd=True) as stream:
            stream.write(encoded)
            stream.flush()
            os.fsync(stream.fileno())
        try:
            os.link(temporary, cached)
        except FileExistsError:
            if read_bounded_regular_file(cached, "cached manifest") != encoded:
                fail("content-addressed cache collision")
    finally:
        temporary.unlink(missing_ok=True)
    return cached


def scan(arguments: list[str]) -> None:
    if len(arguments) != 3:
        fail("usage: scan GHIDRA_INSTALL_DIR CACHE_DIR OUTPUT")
    root = canonical_directory(arguments[0], "GHIDRA_INSTALL_DIR")
    cache = canonical_directory(arguments[1], "CACHE_DIR")
    output = require_new_output(arguments[2])
    encoded = inventory(root)
    cached = cache_inventory(encoded, cache)
    try:
        os.link(cached, output)
    except OSError as error:
        fail(f"cannot publish OUTPUT: {error}")
    file_count = len(json.loads(encoded)["files"])
    print(
        "ghidra-distribution-manifest: "
        f"sha256={hashlib.sha256(encoded).hexdigest()} "
        f"files={file_count} bytes={len(encoded)}"
    )


def verify(arguments: list[str]) -> None:
    if len(arguments) != 2:
        fail("usage: verify GHIDRA_INSTALL_DIR MANIFEST")
    root = canonical_directory(arguments[0], "GHIDRA_INSTALL_DIR")
    manifest = Path(arguments[1])
    expected = read_bounded_regular_file(manifest, "MANIFEST")
    measured = inventory(root)
    if measured != expected:
        fail("Ghidra distribution does not match MANIFEST")
    file_count = len(json.loads(expected)["files"])
    print(
        "ghidra-distribution-manifest: verified "
        f"sha256={hashlib.sha256(expected).hexdigest()} "
        f"files={file_count}"
    )


def main() -> None:
    if len(sys.argv) < 2:
        fail("usage: (scan GHIDRA_INSTALL_DIR CACHE_DIR OUTPUT | verify GHIDRA_INSTALL_DIR MANIFEST)")
    command, arguments = sys.argv[1], sys.argv[2:]
    if command == "scan":
        scan(arguments)
    elif command == "verify":
        verify(arguments)
    else:
        fail(f"unknown command {command!r}")


if __name__ == "__main__":
    main()
