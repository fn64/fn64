#!/usr/bin/env python3
"""Prove both structural lints fail loudly, not with a traceback, on absent input.

Follow-up 5.5b. Both `scripts/lint-writer-channel-topology.py` and
`scripts/lint-compiler-memory-safety.py` hard-code the repository-relative paths
they audit. When four of those paths moved (the 2026-08-15 crate rename and the
`recomps/wm2000` extraction) the lints did not report a missing input: they died
with a `FileNotFoundError` traceback. No CI job ran them, so nobody noticed for
three weeks.

This test pins the replacement contract, for both lints:

  * a root where every required input is present exits 0 and prints the lint's
    own clean/PASS line;
  * a root missing exactly one required input exits 1, names that path in a
    `FATAL:` line on stderr, and prints no traceback.

Stdlib only, builds nothing, runs in well under five seconds.
"""

from __future__ import annotations

import importlib.util
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]

TOPOLOGY = "scripts/lint-writer-channel-topology.py"
MEMORY_SAFETY = "scripts/lint-compiler-memory-safety.py"

# The clean-run line each lint prints on stdout when it has nothing to report.
CLEAN_LINE = {
    TOPOLOGY: "writer-channel topology lint: clean",
    MEMORY_SAFETY: "compiler memory safety lint: PASS",
}


def required_paths(lint: str) -> tuple[str, ...]:
    """Import a lint as a module and read its own list of required inputs.

    Reading the constant rather than restating it here means a future path
    change cannot silently escape this test.
    """
    spec = importlib.util.spec_from_file_location(
        f"_lint_{Path(lint).stem.replace('-', '_')}", REPO_ROOT / lint
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return tuple(str(path) for path in module.REQUIRED)


def run_lint(lint: str, root: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(REPO_ROOT / lint), "--root", str(root)],
        capture_output=True,
        text=True,
        timeout=60,
        check=False,
    )


def populate(root: Path, lint: str, omit: str | None = None) -> None:
    """Copy every required input of `lint` into `root`, optionally omitting one."""
    for relative in required_paths(lint):
        if relative == omit:
            continue
        destination = root / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(REPO_ROOT / relative, destination)


class RequiredInputsExist(unittest.TestCase):
    """Every path the lints declare must exist in this repository right now."""

    def test_every_required_path_is_present(self) -> None:
        for lint in (TOPOLOGY, MEMORY_SAFETY):
            for relative in required_paths(lint):
                with self.subTest(lint=lint, path=relative):
                    self.assertTrue(
                        (REPO_ROOT / relative).is_file(),
                        f"{lint} requires {relative}, which does not exist",
                    )


class CompleteRootPasses(unittest.TestCase):
    def _assert_clean(self, lint: str) -> None:
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            populate(root, lint)
            result = run_lint(lint, root)
            self.assertEqual(
                result.returncode,
                0,
                f"{lint} on a complete root: stdout={result.stdout!r} "
                f"stderr={result.stderr!r}",
            )
            self.assertIn(CLEAN_LINE[lint], result.stdout)

    def test_topology_lint_passes_on_a_complete_root(self) -> None:
        self._assert_clean(TOPOLOGY)

    def test_memory_safety_lint_passes_on_a_complete_root(self) -> None:
        self._assert_clean(MEMORY_SAFETY)


class MissingInputFailsLoudly(unittest.TestCase):
    def _assert_fatal(self, lint: str, omit: str) -> None:
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            populate(root, lint, omit=omit)
            result = run_lint(lint, root)
            self.assertEqual(
                result.returncode,
                1,
                f"{lint} missing {omit}: stdout={result.stdout!r} "
                f"stderr={result.stderr!r}",
            )
            self.assertIn(
                f"{lint}: FATAL: missing {omit} "
                "(moved? update the path constant in this script)",
                result.stderr,
            )
            self.assertNotIn("Traceback", result.stderr)

    def test_topology_lint_reports_each_missing_input(self) -> None:
        for omit in required_paths(TOPOLOGY):
            with self.subTest(missing=omit):
                self._assert_fatal(TOPOLOGY, omit)

    def test_memory_safety_lint_reports_each_missing_input(self) -> None:
        for omit in required_paths(MEMORY_SAFETY):
            with self.subTest(missing=omit):
                self._assert_fatal(MEMORY_SAFETY, omit)


if __name__ == "__main__":
    unittest.main(verbosity=2)
