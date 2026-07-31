#!/usr/bin/env python3
"""Require a Ghidra launcher to belong to the distribution being audited."""

from __future__ import annotations

from pathlib import Path
import sys
from typing import NoReturn


def fail(message: str) -> NoReturn:
    raise SystemExit(f"ghidra launcher: {message}")


def canonical(path_value: str, label: str) -> Path:
    path = Path(path_value)
    if not path.is_absolute():
        fail(f"{label} must be absolute")
    try:
        return path.resolve(strict=True)
    except OSError as error:
        fail(f"could not resolve {label}: {error}")


def main(arguments: list[str]) -> None:
    if len(arguments) not in (3, 4):
        fail(
            "usage: verify-ghidra-launcher.py GHIDRA_INSTALL_DIR "
            "GHIDRA_LAUNCHER [analyzeHeadless|ghidraRun]"
        )
    install = canonical(arguments[1], "GHIDRA_INSTALL_DIR")
    launcher_name = arguments[3] if len(arguments) == 4 else "analyzeHeadless"
    if launcher_name not in ("analyzeHeadless", "ghidraRun"):
        fail("launcher name must be analyzeHeadless or ghidraRun")
    launcher = canonical(arguments[2], "GHIDRA_LAUNCHER")
    if not install.is_dir():
        fail("GHIDRA_INSTALL_DIR is not a directory")
    expected_path = (
        install / "support" / "analyzeHeadless"
        if launcher_name == "analyzeHeadless"
        else install / "ghidraRun"
    )
    expected = canonical(str(expected_path), f"distribution {launcher_name}")
    if launcher != expected:
        fail("GHIDRA_LAUNCHER does not belong to GHIDRA_INSTALL_DIR")
    if Path(arguments[2]).is_symlink() or not launcher.is_file():
        fail("GHIDRA_LAUNCHER must be a regular non-symlink file")
    if not (install / "Ghidra" / "application.properties").is_file():
        fail("Ghidra application.properties is missing")


if __name__ == "__main__":
    main(sys.argv)
