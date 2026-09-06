#!/usr/bin/env python3
"""Regression tests for WM2000 launch artifact selection."""

from __future__ import annotations

import hashlib
import json
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


def install_synthetic_emit(root: Path) -> tuple[dict[str, str], dict[str, Path]]:
    unusual = root / "inputs with spaces\nand-newline"
    fn64 = unusual / "fn64 source"
    source = fn64 / "crates" / "fn64-cpu-runtime" / "src" / "lib.rs"
    source.parent.mkdir(parents=True)
    source.write_bytes(b"synthetic-fn64-source-v1\n")
    subprocess.run(["git", "init", "-q", str(fn64)], check=True)
    subprocess.run(["git", "-C", str(fn64), "add", "."], check=True)
    subprocess.run(
        [
            "git",
            "-C",
            str(fn64),
            "-c",
            "user.name=fn64 synthetic test",
            "-c",
            "user.email=fn64-synthetic@example.invalid",
            "commit",
            "-q",
            "-m",
            "synthetic source",
        ],
        check=True,
    )
    aki = unusual / "aki"
    config = aki / "games" / "NWXE" / "wm2000.toml"
    config.parent.mkdir(parents=True)
    config.write_bytes(b"synthetic-config-v1\n")
    rom = unusual / "synthetic input.rom"
    rom.write_bytes(b"synthetic-rom-v1\n")
    target = unusual / "cargo target"
    driver = target / "release" / "recompile_rom"
    driver.parent.mkdir(parents=True)
    driver.write_bytes(b"synthetic-recompile-rom-v1\n")
    driver.chmod(0o700)
    scratch = unusual / "scratch"
    emit = scratch / "emit1"
    generated = emit / "src" / "lib.rs"
    generated.parent.mkdir(parents=True)
    generated.write_bytes(b"synthetic-generated-tree-v1\n")
    (emit / "Cargo.toml").write_text(
        '[package]\nname = "synthetic-recompiled"\nversion = "0.0.0"\n\n'
        '[dependencies]\nfn64-cpu-runtime = { path = "/stale/path" }\n',
        encoding="utf-8",
    )
    env = os.environ.copy()
    env.update(
        {
            "FN64": str(fn64),
            "AKI": str(aki),
            "ROM": str(rom),
            "SCRATCH": str(scratch),
            "CARGO_TARGET_DIR": str(target),
        }
    )
    env.pop("FN64_SKIP_EMIT", None)
    return env, {
        "config": config,
        "rom": rom,
        "driver": driver,
        "source": source,
        "emit": emit,
        "generated": generated,
        "receipt": emit / ".fn64-private-emit-receipt.v1.json",
    }


def emit_receipt_command(
    cwd: Path, env: dict[str, str], command: str
) -> subprocess.CompletedProcess[str]:
    command_env = env.copy()
    if command == "--check-emit-reuse":
        command_env["FN64_SKIP_EMIT"] = "1"
    return subprocess.run(
        [str(LAUNCHER), command],
        cwd=cwd,
        env=command_env,
        check=False,
        capture_output=True,
        text=True,
    )


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

    def test_emit_receipt_round_trips_paths_with_spaces_and_newlines(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            env, paths = install_synthetic_emit(root)
            recorded = emit_receipt_command(root, env, "--record-emit-receipt")
            verified = emit_receipt_command(root, env, "--check-emit-reuse")
            receipt = paths["receipt"].read_text(encoding="utf-8")
        self.assertEqual(recorded.returncode, 0, recorded.stderr)
        self.assertEqual(verified.returncode, 0, verified.stderr)
        self.assertIn("private emit receipt verified", verified.stdout)
        self.assertNotIn("synthetic-rom-v1", receipt)
        self.assertNotIn("synthetic-config-v1", receipt)

    def test_emit_reuse_rejects_missing_and_malformed_receipts(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            env, paths = install_synthetic_emit(root)
            missing = emit_receipt_command(root, env, "--check-emit-reuse")
            paths["receipt"].write_bytes(b"{not-json\n")
            malformed = emit_receipt_command(root, env, "--check-emit-reuse")
        self.assertNotEqual(missing.returncode, 0)
        self.assertIn("private emit receipt", missing.stderr)
        self.assertNotEqual(malformed.returncode, 0)
        self.assertIn("parse private emit receipt", malformed.stderr)

    def test_emit_reuse_rejects_receipt_self_digest_mutation(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            env, paths = install_synthetic_emit(root)
            recorded = emit_receipt_command(root, env, "--record-emit-receipt")
            self.assertEqual(recorded.returncode, 0, recorded.stderr)
            receipt = json.loads(paths["receipt"].read_text(encoding="utf-8"))
            receipt["generated_tree"]["sha256"] = "0" * 64
            paths["receipt"].write_text(json.dumps(receipt), encoding="utf-8")
            mismatch = emit_receipt_command(root, env, "--check-emit-reuse")
        self.assertNotEqual(mismatch.returncode, 0)
        self.assertIn("self-digest mismatch", mismatch.stderr)

    def test_emit_reuse_rejects_each_exact_input_mismatch(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            env, paths = install_synthetic_emit(root)
            recorded = emit_receipt_command(root, env, "--record-emit-receipt")
            self.assertEqual(recorded.returncode, 0, recorded.stderr)
            cases = (
                ("config", "recompile config mismatch"),
                ("rom", "ROM input mismatch"),
                ("driver", "recompile_rom mismatch"),
            )
            for path_name, message in cases:
                path = paths[path_name]
                original = path.read_bytes()
                path.write_bytes(original[:-1] + bytes([original[-1] ^ 1]))
                mismatch = emit_receipt_command(root, env, "--check-emit-reuse")
                path.write_bytes(original)
                self.assertNotEqual(mismatch.returncode, 0, path_name)
                self.assertIn(message, mismatch.stderr, path_name)

    def test_emit_reuse_rejects_one_bit_generated_tree_mutation(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            env, paths = install_synthetic_emit(root)
            recorded = emit_receipt_command(root, env, "--record-emit-receipt")
            self.assertEqual(recorded.returncode, 0, recorded.stderr)
            generated = paths["generated"]
            original = generated.read_bytes()
            generated.write_bytes(bytes([original[0] ^ 1]) + original[1:])
            mismatch = emit_receipt_command(root, env, "--check-emit-reuse")
        self.assertNotEqual(mismatch.returncode, 0)
        self.assertIn("generated-tree mismatch", mismatch.stderr)

    def test_emit_reuse_binds_exact_dirty_worktree_bytes(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            env, paths = install_synthetic_emit(root)
            source = paths["source"]
            source.write_bytes(b"synthetic-dirty-source-v1\n")
            recorded = emit_receipt_command(root, env, "--record-emit-receipt")
            verified = emit_receipt_command(root, env, "--check-emit-reuse")
            source.write_bytes(b"synthetic-dirty-source-v2\n")
            mismatch = emit_receipt_command(root, env, "--check-emit-reuse")
        self.assertEqual(recorded.returncode, 0, recorded.stderr)
        self.assertEqual(verified.returncode, 0, verified.stderr)
        self.assertNotEqual(mismatch.returncode, 0)
        self.assertIn("source/worktree mismatch", mismatch.stderr)


class BuildFailurePropagationTests(unittest.TestCase):
    """A lane script that cannot build must exit NONZERO.

    Project rule: a gate that cannot produce its artifact fails loudly. The
    regression this pins is real -- `crates/fn64-shell/rs/Cargo.toml` lacked
    `thiserror`/`serde_json` after the thiserror conversion, so the rs-lane
    shell build died with 63 errors and produced no binary.

    A real `cargo build --release` here would cost minutes, and the thing under
    test is the SCRIPT's propagation, not rustc. So `cargo` is stubbed on PATH
    to fail the way a broken manifest fails. That keeps the test honest about
    what it covers: the script's handling of a failing cargo, nothing more.
    """

    def run_with_failing_cargo(self, cwd: Path) -> subprocess.CompletedProcess[str]:
        stub_dir = cwd / "stub-bin"
        stub_dir.mkdir()
        cargo = stub_dir / "cargo"
        cargo.write_text(
            "#!/bin/sh\n"
            "echo 'error[E0433]: failed to resolve: use of undeclared crate `thiserror`' >&2\n"
            "exit 101\n"
        )
        cargo.chmod(0o755)
        env = os.environ.copy()
        env["PATH"] = f"{stub_dir}:{env['PATH']}"
        # Deliberately NOT setting FN64_SKIP_EMIT: with it, the run dies at the
        # emit-receipt check and never reaches a cargo build at all, so the
        # assertions below would pass even with the failure handling deleted.
        # (Verified by mutation: `|| true` on the shell build left that variant
        # green.) Letting emit run puts the FIRST cargo build -- recompile_rom
        # -- in the path of the stub, which is real propagation code.
        env["SCRATCH"] = str(cwd / "scratch")
        return subprocess.run(
            [str(LAUNCHER), "--print-config"],
            cwd=cwd,
            env=env,
            check=False,
            capture_output=True,
            text=True,
        )

    def test_failing_cargo_fails_the_script(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            result = self.run_with_failing_cargo(Path(temporary))
        self.assertNotEqual(
            result.returncode,
            0,
            "play-wm2000.sh exited 0 despite a failing cargo build:\n"
            f"stdout={result.stdout}\nstderr={result.stderr}",
        )

    def test_failure_names_the_step(self) -> None:
        """The operator must be told WHICH step died, not just handed a 101."""
        with tempfile.TemporaryDirectory() as temporary:
            result = self.run_with_failing_cargo(Path(temporary))
        combined = result.stdout + result.stderr
        self.assertIn(
            "[play-wm2000] FATAL: the recompile_rom build FAILED",
            combined,
            combined,
        )

    def test_no_shell_binary_is_selected_after_a_failed_build(self) -> None:
        """A failed build must never fall through to launching a stale binary."""
        with tempfile.TemporaryDirectory() as temporary:
            result = self.run_with_failing_cargo(Path(temporary))
        self.assertNotIn("selected shell:", result.stdout, result.stdout)


class RsLaneManifestMirrorTests(unittest.TestCase):
    """The rs manifest must carry every workspace-manifest dependency.

    This is the invariant whose violation broke the lane. `cargo test` cannot
    catch it -- nothing in the main workspace builds the rs manifest -- so the
    check is asserted here as well as in scripts/lint-rs-lane-manifest.py.
    """

    def test_rs_manifest_mirrors_the_shell_manifest(self) -> None:
        lint = ROOT / "scripts" / "lint-rs-lane-manifest.py"
        result = subprocess.run(
            ["python3", str(lint)], check=False, capture_output=True, text=True
        )
        self.assertEqual(
            result.returncode, 0, result.stdout + result.stderr
        )

    def test_the_mirror_lint_self_test_passes(self) -> None:
        lint = ROOT / "scripts" / "lint-rs-lane-manifest.py"
        result = subprocess.run(
            ["python3", str(lint), "--self-test"],
            check=False,
            capture_output=True,
            text=True,
        )
        self.assertEqual(
            result.returncode, 0, result.stdout + result.stderr
        )


if __name__ == "__main__":
    unittest.main()
