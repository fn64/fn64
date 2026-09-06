#!/usr/bin/env python3
"""Tests for scripts/knob-registry.py: the three failure modes the task
brief names -- an unclassified name fails, a stale knobs.toml entry fails,
and --write followed by a check round-trips clean.

Loads the script as a module and points its ROOT/KNOBS_TOML/GENERATED_DOC
module globals at a scratch directory tree, so each test runs the real
scan/parse/render/compare logic without touching the checked-in
docs/knobs.toml or docs/RUNTIME-KNOBS.md.
"""
from __future__ import annotations

import contextlib
import importlib.util
import io
import shutil
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT_PATH = Path(__file__).resolve().parent / "knob-registry.py"


def _load_module():
    spec = importlib.util.spec_from_file_location("knob_registry", SCRIPT_PATH)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class KnobRegistryTest(unittest.TestCase):
    def setUp(self) -> None:
        self.kr = _load_module()
        self.tmp = Path(tempfile.mkdtemp(prefix="knob-registry-test-"))
        self.addCleanup(shutil.rmtree, self.tmp, ignore_errors=True)
        (self.tmp / "crates/fn64-fake/src").mkdir(parents=True)
        (self.tmp / "docs").mkdir()
        (self.tmp / "crates/fn64-fake/src/lib.rs").write_text(
            'fn enabled() -> bool {\n'
            '    std::env::var_os("FN64_FAKE_KNOB").is_some()\n'
            '}\n'
        )
        # A sibling file whose NAME contains "tests" must be excluded from
        # the scan (the task contract excludes any path containing "tests",
        # not just a tests/ directory) -- give it a name absent from real
        # source, so a scanner regression that wrongly includes it would
        # only be caught here, not by the round-trip test alone.
        (self.tmp / "crates/fn64-fake/src/lib_tests.rs").write_text(
            'fn only_in_tests() { std::env::var_os("FN64_TESTS_ONLY_KNOB"); }\n'
        )
        self.kr.ROOT = self.tmp
        self.kr.KNOBS_TOML = self.tmp / "docs/knobs.toml"
        self.kr.GENERATED_DOC = self.tmp / "docs/RUNTIME-KNOBS.md"

    def _write_knobs_toml(self, body: str) -> None:
        self.kr.KNOBS_TOML.write_text(body)

    def _run(self, argv: list[str]) -> tuple[int, str, str]:
        old_argv = sys.argv
        sys.argv = ["knob-registry.py", *argv]
        out, err = io.StringIO(), io.StringIO()
        try:
            with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
                code = self.kr.main()
        finally:
            sys.argv = old_argv
        return code, out.getvalue(), err.getvalue()

    def test_unknown_name_fails(self) -> None:
        """A name read in code but missing from knobs.toml fails the script."""
        self._write_knobs_toml("")
        code, _out, err = self._run([])
        self.assertNotEqual(code, 0)
        self.assertIn("FN64_FAKE_KNOB", err)
        self.assertIn("unclassified", err)

    def test_stale_name_fails(self) -> None:
        """A knobs.toml entry whose name no longer appears in code fails."""
        self._write_knobs_toml(
            '[FN64_FAKE_KNOB]\n'
            'class = "user"\n'
            'note = "test fixture"\n'
            '\n'
            '[FN64_LONG_GONE]\n'
            'class = "dead"\n'
            'note = "this name was deleted from source"\n'
        )
        code, _out, err = self._run([])
        self.assertNotEqual(code, 0)
        self.assertIn("FN64_LONG_GONE", err)
        self.assertIn("stale", err)

    def test_write_then_check_round_trips(self) -> None:
        """--write regenerates the doc; a subsequent check-mode run is clean."""
        self._write_knobs_toml(
            '[FN64_FAKE_KNOB]\n'
            'class = "user"\n'
            'note = "test fixture"\n'
        )
        write_code, _out, write_err = self._run(["--write"])
        self.assertEqual(write_code, 0, write_err)
        self.assertTrue(self.kr.GENERATED_DOC.exists())

        check_code, check_out, check_err = self._run([])
        self.assertEqual(check_code, 0, check_err)
        self.assertIn("clean", check_out)

        # The excluded tests-file name must never reach the generated doc.
        doc_text = self.kr.GENERATED_DOC.read_text()
        self.assertIn("FN64_FAKE_KNOB", doc_text)
        self.assertNotIn("FN64_TESTS_ONLY_KNOB", doc_text)


if __name__ == "__main__":
    unittest.main()
