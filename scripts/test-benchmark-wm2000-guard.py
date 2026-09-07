#!/usr/bin/env python3
"""Test the contention guard in scripts/benchmark-wm2000-render.zsh.

Stubs `ps` on PATH and runs the script's `--check-contention` dry-run entry
point (no ROM, no built shell needed) to prove the guard refuses runs when a
cargo/rustc build or an fn64/merciless GPU process is active, matched by
basename only.

Stdlib-only, no network, expected to run in well under 5 seconds.
"""

import os
import stat
import subprocess
import sys
import tempfile
import textwrap
import unittest

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SCRIPT_PATH = os.path.join(REPO_ROOT, "scripts", "benchmark-wm2000-render.zsh")


def _write_ps_stub(directory: str, output: str) -> None:
    ps_path = os.path.join(directory, "ps")
    with open(ps_path, "w") as f:
        f.write("#!/bin/sh\n")
        f.write(f"printf '{output}'\n")
    st = os.stat(ps_path)
    os.chmod(ps_path, st.st_mode | stat.S_IEXEC | stat.S_IXGRP | stat.S_IXOTH)


def _run_with_stub_ps(ps_output: str):
    with tempfile.TemporaryDirectory() as stub_dir:
        _write_ps_stub(stub_dir, ps_output)
        env = dict(os.environ)
        env["PATH"] = stub_dir + os.pathsep + env.get("PATH", "")
        result = subprocess.run(
            ["zsh", SCRIPT_PATH, "--check-contention"],
            capture_output=True,
            text=True,
            env=env,
            timeout=5,
        )
        return result


class BenchmarkContentionGuardTest(unittest.TestCase):
    def test_uses_unqualified_ps(self):
        # If the script hardcodes /bin/ps (or another absolute path) instead
        # of calling `ps` unqualified, PATH stubbing has no effect and every
        # other test in this file would be exercising the real, unstubbed
        # `ps` -- silently passing for the wrong reason. Guard against that.
        with open(SCRIPT_PATH) as f:
            contents = f.read()
        self.assertIn(
            "ps -Ao",
            contents,
            "expected the script to call `ps` unqualified so PATH stubbing "
            "works; found no unqualified `ps -Ao` invocation",
        )
        self.assertNotIn(
            "/bin/ps",
            contents,
            "script calls an absolute ps path, which PATH stubbing cannot "
            "intercept",
        )

    def test_quiet_machine_exits_zero(self):
        result = _run_with_stub_ps("  1 /sbin/launchd\\n 42 zsh\\n 77 python3\\n")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(
            "benchmark-wm2000: no heavy processes; the machine is quiet "
            "enough to benchmark",
            result.stdout,
        )

    def test_foreign_merciless_game_refuses(self):
        result = _run_with_stub_ps(
            "  1 /sbin/launchd\\n 42 zsh\\n"
            "501 /Users/x/target/release/merciless-game\\n"
        )
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("refusing a contended run: 1 heavy", result.stderr)
        self.assertIn("501 /Users/x/target/release/merciless-game", result.stderr)
        self.assertIn(
            "foreign GPU work skews swaps by up to 8 ms mean (2026-09-06)",
            result.stderr,
        )

    def test_basename_only_match_excludes_lookalike(self):
        result = _run_with_stub_ps(
            "900 /opt/bin/fn64\\n901 rustc\\n902 /x/fn64-not-this\\n"
        )
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("refusing a contended run: 2 heavy", result.stderr)
        self.assertNotIn("fn64-not-this", result.stderr)

    def test_cargo_nextest_is_not_a_basename_match(self):
        result = _run_with_stub_ps("903 cargo-nextest\\n")
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn(
            "benchmark-wm2000: no heavy processes; the machine is quiet "
            "enough to benchmark",
            result.stdout,
        )


if __name__ == "__main__":
    unittest.main()
