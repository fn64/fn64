#!/usr/bin/env python3
"""Regression tests for WM2000 launch artifact selection."""

from __future__ import annotations

import hashlib
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parent.parent
LAUNCHER = ROOT / "scripts" / "play-wm2000.sh"


def artifact_paths(cwd: Path, target: str | None) -> dict[str, Path]:
    env = os.environ.copy()
    if target is None:
        env.pop("CARGO_TARGET_DIR", None)
    else:
        env["CARGO_TARGET_DIR"] = target
    result = subprocess.run(
        [str(LAUNCHER), "--print-artifact-paths"],
        cwd=cwd,
        env=env,
        check=True,
        capture_output=True,
        text=True,
    )
    return {
        key: Path(value)
        for key, value in (line.split("=", 1) for line in result.stdout.splitlines())
    }


def shell_reuse(cwd: Path, target: str, expected: str | None) -> subprocess.CompletedProcess[str]:
    env = os.environ.copy()
    env["CARGO_TARGET_DIR"] = target
    env["FN64_SKIP_SHELL_BUILD"] = "1"
    if expected is None:
        env.pop("FN64_EXPECT_SHELL_SHA256", None)
    else:
        env["FN64_EXPECT_SHELL_SHA256"] = expected
    return subprocess.run(
        [str(LAUNCHER), "--check-shell-reuse"],
        cwd=cwd,
        env=env,
        check=False,
        capture_output=True,
        text=True,
    )


def install_synthetic_shell(target: Path, contents: bytes = b"synthetic shell\n") -> tuple[Path, str]:
    shell = target / "release" / "fn64"
    shell.parent.mkdir(parents=True)
    shell.write_bytes(contents)
    shell.chmod(0o700)
    return shell, hashlib.sha256(contents).hexdigest()


class ArtifactPathTests(unittest.TestCase):
    def test_default_targets_match_each_cargo_root(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            paths = artifact_paths(Path(temporary), None)
        self.assertEqual(paths["recompile_rom"], ROOT / "target/release/recompile_rom")
        self.assertEqual(
            paths["shell"], ROOT / "crates/fn64-shell/rs/target/release/fn64"
        )

    def test_absolute_override_selects_both_built_artifacts(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            target = Path(temporary) / "cargo-target"
            paths = artifact_paths(ROOT, str(target))
        canonical_target = target.resolve()
        self.assertEqual(
            paths["recompile_rom"], canonical_target / "release/recompile_rom"
        )
        self.assertEqual(paths["shell"], canonical_target / "release/fn64")

    def test_relative_override_is_bound_to_invocation_directory(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            cwd = Path(temporary)
            paths = artifact_paths(cwd, "relative-target")
        target = (cwd / "relative-target/release").resolve()
        self.assertEqual(paths["recompile_rom"], target / "recompile_rom")
        self.assertEqual(paths["shell"], target / "fn64")

    def test_absolute_target_reuses_only_the_expected_shell_bytes(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            target = root / "absolute-target"
            shell, digest = install_synthetic_shell(target)
            result = shell_reuse(root, str(target), digest)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(str(shell), result.stdout)
        self.assertIn(f"sha256={digest}", result.stdout)

    def test_relative_target_reuses_only_the_expected_shell_bytes(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            target = root / "relative-target"
            _, digest = install_synthetic_shell(target)
            result = shell_reuse(root, "relative-target", digest)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_reuse_rejects_digest_mismatch(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            target = root / "target"
            install_synthetic_shell(target)
            result = shell_reuse(root, str(target), "0" * 64)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("reused shell digest mismatch", result.stderr)

    def test_reuse_rejects_missing_or_malformed_digest(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            target = root / "target"
            install_synthetic_shell(target)
            missing = shell_reuse(root, str(target), None)
            malformed = shell_reuse(root, str(target), "A" * 64)
        self.assertNotEqual(missing.returncode, 0)
        self.assertIn("requires FN64_EXPECT_SHELL_SHA256", missing.stderr)
        self.assertNotEqual(malformed.returncode, 0)
        self.assertIn("64 lowercase hexadecimal", malformed.stderr)

    def test_reuse_rejects_symlink_even_when_bytes_match(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            target = root / "target"
            real_shell, digest = install_synthetic_shell(root / "real")
            shell = target / "release" / "fn64"
            shell.parent.mkdir(parents=True)
            shell.symlink_to(real_shell)
            result = shell_reuse(root, str(target), digest)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("regular, non-symlink executable", result.stderr)


if __name__ == "__main__":
    unittest.main()
