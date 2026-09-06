#!/usr/bin/env python3
"""Tests for scripts/knob-registry.py: the failure modes the task brief
names -- an unclassified name fails, a stale knobs.toml entry fails,
--write followed by a check round-trips clean, and a classification that
contradicts its read kind (build-time vs. runtime) fails.

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
            'const PINNED: &str = env!("FN64_FAKE_BUILD_TIME_KNOB");\n'
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

    # Both source knobs (FN64_FAKE_KNOB via env::var_os, FN64_FAKE_BUILD_TIME_KNOB
    # via env!()) classified consistently with their read kind -- the shared
    # "everything is fine" baseline each test starts from or mutates.
    VALID_TOML = (
        '[FN64_FAKE_KNOB]\n'
        'class = "user"\n'
        'note = "test fixture"\n'
        '\n'
        '[FN64_FAKE_BUILD_TIME_KNOB]\n'
        'class = "build-time"\n'
        'note = "test fixture, env!() only"\n'
    )

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
            self.VALID_TOML
            + '\n[FN64_LONG_GONE]\n'
            'class = "dead"\n'
            'note = "this name was deleted from source"\n'
        )
        code, _out, err = self._run([])
        self.assertNotEqual(code, 0)
        self.assertIn("FN64_LONG_GONE", err)
        self.assertIn("stale", err)

    def test_write_then_check_round_trips(self) -> None:
        """--write regenerates the doc; a subsequent check-mode run is clean."""
        self._write_knobs_toml(self.VALID_TOML)
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
        # The read-kind column reflects each name's actual call shape.
        self.assertIn("| runtime |", doc_text)
        self.assertIn("| build-time |", doc_text)
        # The "First site" column is a file path, not `file:line`: a line
        # number made every read-site-shifting refactor stale this doc even
        # when no knob's existence, name, or classification changed. Assert
        # the exact path with no trailing `:<digits>`, so a regression that
        # re-adds line tracking fails here rather than only in a real
        # refactor's line-shift days later.
        self.assertIn("`crates/fn64-fake/src/lib.rs`", doc_text)
        self.assertNotRegex(doc_text, r"crates/fn64-fake/src/lib\.rs:\d+")

    def test_occurrence_has_no_line_attribute(self) -> None:
        """`Occurrence` tracks a file path only -- no per-name line number.
        A future edit that reintroduces line tracking on the scan side
        without updating render_doc/error messages should fail here, not
        silently reappear in the generated doc."""
        self.assertNotIn("line", self.kr.Occurrence.__slots__)

    def test_read_kind_mismatch_fails_both_directions(self) -> None:
        """A class that contradicts its read kind fails: build-time claimed
        for a name with a runtime read, and a non-build-time class claimed
        for a name with ONLY build-time reads."""
        # Direction 1: FN64_FAKE_KNOB has a runtime env::var_os read site but
        # is (wrongly) classified build-time.
        self._write_knobs_toml(
            '[FN64_FAKE_KNOB]\n'
            'class = "build-time"\n'
            'note = "wrongly claims build-time despite a runtime read"\n'
            '\n'
            '[FN64_FAKE_BUILD_TIME_KNOB]\n'
            'class = "build-time"\n'
            'note = "test fixture, env!() only"\n'
        )
        code, _out, err = self._run([])
        self.assertNotEqual(code, 0)
        self.assertIn("FN64_FAKE_KNOB", err)
        self.assertIn("runtime", err)

        # Direction 2: FN64_FAKE_BUILD_TIME_KNOB has ONLY an env!() read site
        # but is (wrongly) classified user.
        self._write_knobs_toml(
            '[FN64_FAKE_KNOB]\n'
            'class = "user"\n'
            'note = "test fixture"\n'
            '\n'
            '[FN64_FAKE_BUILD_TIME_KNOB]\n'
            'class = "user"\n'
            'note = "wrongly claims user despite build-time-only reads"\n'
        )
        code, _out, err = self._run([])
        self.assertNotEqual(code, 0)
        self.assertIn("FN64_FAKE_BUILD_TIME_KNOB", err)
        self.assertIn("build-time", err)


if __name__ == "__main__":
    unittest.main()
