#!/usr/bin/env python3
"""Tests for lint-docs' section-anchor and symbol-citation checks.

These two checks exist to catch a doc that points somewhere that no longer
exists -- a `§7` after section 7 was renumbered, a `build.rs:215-245` after
anyone edited build.rs. A check like that is worthless unless it can FAIL, so
every case below is built to make exactly one of them fire (or, for the
negative cases, to prove it stays quiet on prose that is correct).

The pair that matters most:
  * `## Task 7` must NOT satisfy `§7` -- 89 headings in this repo start with a
    word and a digit ("Phase 1", "Task 0.3"). If those counted, nearly every
    anchor would resolve and the check would prove nothing.
  * a symbol citation is checked with word boundaries, so `run` is not
    satisfied by `run_all` -- substring matching is the false confidence the
    symbol form is supposed to replace.

Run: python3 scripts/test-lint-docs-anchors.py    (stdlib only, ~0.1 s)
"""
import importlib.util
import tempfile
import unittest
from pathlib import Path

_SPEC = importlib.util.spec_from_file_location(
    "lint_docs", Path(__file__).resolve().parent / "lint-docs.py"
)
lint_docs = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(lint_docs)

check_section_anchors = lint_docs.check_section_anchors
check_symbol_citations = lint_docs.check_symbol_citations


class TempRepo:
    """A throwaway directory of docs; the checks take a root, so no git needed."""

    def __init__(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)

    def write(self, rel, text):
        p = self.root / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(text)
        return p

    def close(self):
        self._tmp.cleanup()


class AnchorTests(unittest.TestCase):
    def setUp(self):
        self.repo = TempRepo()
        self.addCleanup(self.repo.close)

    def test_cross_doc_anchor_resolves(self):
        self.repo.write("docs/TARGET.md", "# T\n\n## 7. What CI proves\n")
        self.repo.write("docs/SRC.md", "See `TARGET.md` §7 for the gate.\n")
        self.assertEqual(check_section_anchors(self.repo.root), [])

    def test_cross_doc_anchor_that_does_not_resolve_is_an_error(self):
        self.repo.write("docs/TARGET.md", "# T\n\n## 7. What CI proves\n")
        self.repo.write("docs/SRC.md", "See `TARGET.md` §9 for the gate.\n")
        errors = check_section_anchors(self.repo.root)
        self.assertEqual(len(errors), 1, errors)
        self.assertIn("§9 does not resolve", errors[0])
        self.assertIn("docs/SRC.md:1", errors[0])

    def test_chained_anchors_share_one_doc_reference(self):
        """`X.md` §7 and §7.1 -- the second must see the same target."""
        self.repo.write("docs/TARGET.md", "# T\n\n## 7. A\n\n### 7.1 B\n")
        self.repo.write("docs/SRC.md", "(`TARGET.md` §7 and §7.1).\n")
        self.assertEqual(check_section_anchors(self.repo.root), [])

    def test_chained_anchor_reports_only_the_missing_one(self):
        self.repo.write("docs/TARGET.md", "# T\n\n## 7. A\n")
        self.repo.write("docs/SRC.md", "(`TARGET.md` §7 and §7.4).\n")
        errors = check_section_anchors(self.repo.root)
        self.assertEqual(len(errors), 1, errors)
        self.assertIn("§7.4", errors[0])

    def test_same_doc_anchor_resolves_against_its_own_headings(self):
        self.repo.write("docs/SELF.md", "# S\n\n## 3. Memory model\n\nPer §3 above.\n")
        self.assertEqual(check_section_anchors(self.repo.root), [])

    def test_same_doc_anchor_missing_is_an_error(self):
        self.repo.write("docs/SELF.md", "# S\n\n## 3. Memory model\n\nPer §3.1 above.\n")
        errors = check_section_anchors(self.repo.root)
        self.assertEqual(len(errors), 1, errors)
        self.assertIn("§3.1 does not resolve", errors[0])

    def test_task_heading_does_not_satisfy_a_number_anchor(self):
        """`## Task 7` is prose, not section 7. This is the whole point."""
        self.repo.write("docs/TARGET.md", "# T\n\n## Task 7: land it\n")
        self.repo.write("docs/SRC.md", "See `TARGET.md` §7.\n")
        errors = check_section_anchors(self.repo.root)
        self.assertEqual(len(errors), 1, errors)
        self.assertIn("§7 does not resolve", errors[0])

    def test_phase_heading_does_not_satisfy_a_number_anchor(self):
        self.repo.write("docs/SELF.md", "# S\n\n## Phase 1: build\n\nPer §1.\n")
        errors = check_section_anchors(self.repo.root)
        self.assertEqual(len(errors), 1, errors)

    def test_heading_number_with_and_without_trailing_dot_both_resolve(self):
        self.repo.write("docs/A.md", "# A\n\n## 7 No dot\n\n### 7.1 Nested\n\nSee §7 and §7.1.\n")
        self.assertEqual(check_section_anchors(self.repo.root), [])

    def test_letter_suffixed_subsection_resolves(self):
        """`### 3a. ...` is a real shape here (36 such headings)."""
        self.repo.write("docs/A.md", "# A\n\n### 3a. The busy-spin\n\nSee §3a.\n")
        self.assertEqual(check_section_anchors(self.repo.root), [])

    def test_ambiguous_basename_is_an_error_naming_the_candidates(self):
        self.repo.write("docs/one/DUP.md", "# D\n\n## 1. A\n")
        self.repo.write("docs/two/DUP.md", "# D\n\n## 1. A\n")
        self.repo.write("docs/SRC.md", "See `DUP.md` §1.\n")
        errors = check_section_anchors(self.repo.root)
        self.assertEqual(len(errors), 1, errors)
        self.assertIn("ambiguous", errors[0])
        self.assertIn("docs/one/DUP.md", errors[0])
        self.assertIn("docs/two/DUP.md", errors[0])

    def test_external_referent_is_not_reported(self):
        """`Chapter 15 §15.7` cites the RDP manual; it cannot resolve here."""
        self.repo.write(
            "docs/A.md",
            "# A\n\n## 1. X\n\nCited to Programming Manual §15.5.4, and\n"
            "the port card's §4 as well.\n",
        )
        self.assertEqual(check_section_anchors(self.repo.root), [])

    def test_superseded_doc_is_skipped(self):
        self.repo.write("docs/OLD.md", "# Old\n\n> **SUPERSEDED** by X\n\nSee §9.\n")
        self.assertEqual(check_section_anchors(self.repo.root), [])


class SymbolCitationTests(unittest.TestCase):
    def setUp(self):
        self.repo = TempRepo()
        self.addCleanup(self.repo.close)

    def test_bare_line_citation_is_an_error(self):
        self.repo.write("docs/plans/perf-method.md", "See `build.rs:215-245` for it.\n")
        errors = check_symbol_citations(self.repo.root, converted=("perf-method.md",))
        self.assertEqual(len(errors), 1, errors)
        self.assertIn("bare line citation; cite the symbol", errors[0])

    def test_bare_single_line_citation_is_an_error(self):
        self.repo.write("docs/plans/perf-method.md", "At `shell.rs:903` it is set.\n")
        errors = check_symbol_citations(self.repo.root, converted=("perf-method.md",))
        self.assertEqual(len(errors), 1, errors)

    def test_bare_line_citation_outside_the_converted_docs_is_not_reported(self):
        """The rule guards the docs actually converted; see LINE_CITATION_FREE."""
        self.repo.write("docs/plans/other-plan.md", "See `build.rs:215-245`.\n")
        self.assertEqual(check_symbol_citations(self.repo.root), [])

    def test_symbol_citation_present_as_whole_word_is_ok(self):
        self.repo.write("crates/x/src/lib.rs", "pub fn run_all() {}\npub fn run() {}\n")
        self.repo.write(
            "docs/plans/p.md", "`run` in `crates/x/src/lib.rs` does it.\n"
        )
        self.assertEqual(check_symbol_citations(self.repo.root), [])

    def test_symbol_present_only_as_a_substring_is_an_error(self):
        """`run` must NOT be satisfied by `run_all`."""
        self.repo.write("crates/x/src/lib.rs", "pub fn run_all() {}\n")
        self.repo.write(
            "docs/plans/p.md", "`run` in `crates/x/src/lib.rs` does it.\n"
        )
        errors = check_symbol_citations(self.repo.root)
        self.assertEqual(len(errors), 1, errors)
        self.assertIn("which does not contain it", errors[0])

    def test_symbol_citation_with_missing_path_is_an_error(self):
        self.repo.write("docs/plans/p.md", "`thing` in `crates/x/src/gone.rs`.\n")
        errors = check_symbol_citations(self.repo.root)
        self.assertEqual(len(errors), 1, errors)
        self.assertIn("which does not exist", errors[0])

    def test_bare_basename_is_not_treated_as_a_path_claim(self):
        """`metadata.rs` is generated at build time; it is not on disk."""
        self.repo.write("docs/plans/p.md", "`WORDS` in `metadata.rs` is emitted.\n")
        self.assertEqual(check_symbol_citations(self.repo.root), [])

    def test_python_and_shell_paths_are_checked_too(self):
        self.repo.write("scripts/tool.py", "def helper():\n    pass\n")
        self.repo.write("docs/plans/p.md", "`missing_fn` in `scripts/tool.py`.\n")
        errors = check_symbol_citations(self.repo.root)
        self.assertEqual(len(errors), 1, errors)


if __name__ == "__main__":
    unittest.main(verbosity=2)
