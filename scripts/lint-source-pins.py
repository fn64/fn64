#!/usr/bin/env python3
"""lint-source-pins -- a test that quotes source must still be quoting it.

A "source pin" is a test that reads its own crate's source text back with
`include_str!("<file>")` and then asserts a string-literal NEEDLE still
appears in it -- `.find("fn process_task")`, `.contains("self.commit()")`,
the bounded `method_source(...)` helper in
`crates/fn64-render-rt64/src/tests.rs`, and `production.rs`'s six pins in
`fn64-render-wgpu`. These exist because a behavioral assertion cannot express
"this exact call still appears in this exact function" -- only a source
grep can -- so when a structural refactor (a file split, a rename, a method
moved between impls) moves the needle, the pin does not fail with a message
that names what moved. It fails as an ordinary assertion, or -- worse, if the
needle's surrounding text still happens to contain a substring match --
it silently widens to match something the author never intended and stops
proving anything at all. Task 2.4 did exactly the first of those to one pin
and the second to another, in the same PR, both unnoticed until a human went
looking by hand.

This script is that hand-look, mechanized: for every `include_str!("<file>")`
in a test-bearing source file, find every `.find("<needle>")` and
`.contains("<needle>")` call in the same function body, resolve `<file>`
relative to the file containing the macro, and confirm the LITERAL needle
still occurs in the resolved file's current text. A needle built from a
variable, `format!`, or `concat!` cannot be checked this way -- those are
reported (not failed) as "computed, not checked" so the gap is visible
rather than silently swallowed.

Usage: scripts/lint-source-pins.py           (exit 0 clean, 1 on any broken pin)
       scripts/lint-source-pins.py --self-test

ponytail: one regex pass per test file, no AST, no full data-flow -- but
enough name tracking to matter. A function that pins two different
`include_str!` targets (`fn64-render-rt64/src/ffi/tests.rs` pins
`fn64_rt64_shim.cpp`, `CMakeLists.txt`, and three more headers in ONE test)
must not credit a needle checked against `cmake` as though it were checked
against `shim` -- so each `let VAR = include_str!(...)` binding is tracked by
NAME, `VAR.find(...)`/`VAR.contains(...)` calls are matched to the binding
they were actually called on, and one further hop is followed for a slice
derived from that binding (`let body = &source[a..b];` / `source[start..]` /
`source.split(...)`) so the `method_source`/`body`/`fields`-style pins in
`production.rs` and `fn64-render-rt64/src/tests.rs` still resolve. A needle
called on a name this script cannot trace back to an `include_str!` binding
is not a pin at all and is silently skipped, not reported -- most `.find`/
`.contains` calls in the tree have nothing to do with a source pin.
"""
from __future__ import annotations

import argparse
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent

# Only files git actually tracks are scanned; `target/` and friends are noise
# and may contain generated `.rs` this lint has no business checking.

# A `fn` header, used to find the enclosing function's start and its sibling
# (the next `fn` at the same or shallower indent, or the file's end).
FN_HEADER = re.compile(
    r'(?m)^(?P<indent>[ \t]*)(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:unsafe\s+)?'
    r'fn\s+(?P<name>[A-Za-z0-9_]+)\b'
)

# A binding directly to `include_str!`: `let source = include_str!("...")`,
# with or without `&`/`ref`/a type annotation. The identifier is the pin's
# root name.
INCLUDE_BINDING = re.compile(
    r'let\s+(?:mut\s+)?(?P<name>[A-Za-z_][A-Za-z0-9_]*)\s*(?::\s*[^=]+)?=\s*'
    r'include_str!\(\s*"(?P<path>[^"]+)"\s*\)'
)

# A one-hop derived binding: `let body = &source[a..b];`, `let block =
# rest[..end];`, `let arm = source.split("...")...;` -- anything whose
# right-hand side visibly names an already-tracked identifier. This is a
# single hop, not transitive closure through arbitrary expressions: it is
# enough to resolve the `body`/`fields`/`arm`/`block` shapes actually in the
# tree (`production.rs`'s six pins, `fn64-render-rt64/src/tests.rs`'s
# `method_source` helper, `raw_dpc/mod.rs`'s `planning_surface`) without
# claims about code this lint has never seen.
DERIVED_BINDING = re.compile(
    r'let\s+(?:mut\s+)?(?P<name>[A-Za-z_][A-Za-z0-9_]*)\s*(?::\s*[^=]+)?=\s*'
    r'(?P<rhs>[^;]*?)\s*;'
)

# `VAR.find("literal")` / `VAR.contains("literal")` -- the two call shapes
# the task brief names, called directly on a tracked identifier (optionally
# through `&`). `.matches(` (used for a count assertion elsewhere in the
# tree) is deliberately out of scope: the brief specifies find/contains only.
NEEDLE_CALL = re.compile(
    r'(?P<var>[A-Za-z_][A-Za-z0-9_]*)\s*\.\s*(?P<kind>find|contains)\('
    r'\s*"(?P<needle>(?:[^"\\]|\\.)*)"\s*\)'
)

# The same call shape but with a NON-literal argument -- a bare identifier,
# `format!(...)`, `concat!(...)`, or anything else that isn't a plain string
# literal. Reported, never failed: the task brief says these are not
# checkable this way, and a lint that guessed at their value would be
# guessing, not checking.
NEEDLE_CALL_COMPUTED = re.compile(
    r'(?P<var>[A-Za-z_][A-Za-z0-9_]*)\s*\.\s*(?P<kind>find|contains)\(\s*((?!")[^)]*)\)'
)


def unescape(literal: str) -> str:
    r"""Turn a Rust string-literal body (already stripped of its quotes) into
    the text it denotes.

    Must be a SINGLE left-to-right pass, not sequential whole-string
    `.replace()` calls: `\\n` (an escaped backslash followed by a literal
    `n`) must decode to the two characters `\` and `n`, never to a newline --
    exactly the needle `fn64-render-rt64/src/ffi/tests.rs:583` uses to check
    that a C++ source string still embeds a literal `\n`. Sequential
    `.replace("\\\\", "\\")` before `.replace("\\n", "\n")` gets this
    backwards: it turns the escaped backslash into a bare one FIRST, which
    the next replace then reads as a newline escape it never was.
    """
    out: list[str] = []
    i = 0
    while i < len(literal):
        ch = literal[i]
        if ch == "\\" and i + 1 < len(literal):
            nxt = literal[i + 1]
            if nxt == "n":
                out.append("\n")
                i += 2
                continue
            if nxt == "t":
                out.append("\t")
                i += 2
                continue
            if nxt in ('"', "\\"):
                out.append(nxt)
                i += 2
                continue
        out.append(ch)
        i += 1
    return "".join(out)


def enclosing_function_span(source: str, offset: int) -> tuple[int, int, str]:
    """(start, end, name) of the `fn` whose body contains `offset`.

    `end` is found by brace-matching the function's own opening `{`, not by
    an indent heuristic: a same-or-lesser-indent "next sibling fn" search
    was tried first and is WRONG the moment a helper function is followed by
    a `#[cfg(test)] mod tests { ... }` block, which is the ordinary shape of
    every file in this tree that has both production helpers and inline
    tests. `fn64-render-wgpu/src/production.rs`'s 0-indent `pixel_size`
    helper (line 10126) is followed by no further 0-indent `fn` for over
    12,000 lines -- the whole `mod tests` block sits at 4-space indent --
    so the indent heuristic credited its enclosing "function" with every
    include_str! and needle call in the entire test module, silently
    crediting one function's pins to needles that were actually written
    against a different `include_str!` target several thousand lines away.
    Brace matching has no such failure mode: it stops exactly where the
    function's own body closes, regardless of what follows it.
    """
    headers = list(FN_HEADER.finditer(source))
    start = 0
    name = "<module scope>"
    for match in headers:
        if match.start() > offset:
            break
        start = match.start()
        name = match.group("name")
    if start == 0 and name == "<module scope>":
        return 0, len(source), name
    open_brace = source.find("{", start)
    if open_brace == -1:
        return start, len(source), name
    end = _match_brace(source, open_brace)
    if end == -1:
        end = len(source)
    return start, end, name


def _match_brace(source: str, open_brace: int) -> int:
    """Index one past the `}` matching `source[open_brace]` (which must be
    `{`), skipping braces inside string/char literals and comments so a
    source pin's own quoted Rust snippets (`"fn f() { ... }"`, common in
    this tree's `method_source`-style tests) never desynchronize the count.
    Returns -1 if unmatched (malformed input; caller falls back to EOF).
    """
    depth = 0
    i = open_brace
    n = len(source)
    while i < n:
        ch = source[i]
        if ch == "/" and i + 1 < n and source[i + 1] == "/":
            i = source.find("\n", i)
            if i == -1:
                return -1
            continue
        if ch == "/" and i + 1 < n and source[i + 1] == "*":
            close = source.find("*/", i + 2)
            i = n if close == -1 else close + 2
            continue
        if ch == '"':
            i += 1
            while i < n and source[i] != '"':
                i += 2 if source[i] == "\\" else 1
            i += 1
            continue
        if ch == "'":
            # A char literal (`'a'`, `'\''`, `'\n'`) or a lifetime (`'a`).
            # Only consume as a char literal when it actually closes with a
            # matching quote within a few characters; otherwise it's a
            # lifetime and the quote itself is not a delimiter.
            j = i + 1
            if j < n and source[j] == "\\":
                j += 2
            else:
                j += 1
            if j < n and source[j] == "'":
                i = j + 1
                continue
            i += 1
            continue
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
            if depth == 0:
                return i + 1
        i += 1
    return -1


def _display_path(path: pathlib.Path) -> str:
    """`path` relative to ROOT when it is under ROOT (the normal case); the
    path itself otherwise (self-test fixtures live in a scratch tempdir)."""
    try:
        return str(path.relative_to(ROOT))
    except ValueError:
        return str(path)


def _is_negated(body: str, match_start: int) -> bool:
    """True if a `!` immediately precedes this call chain's start (skipping
    only whitespace) -- `!source.contains("X")`, asserting the needle's
    ABSENCE rather than its presence.

    Deliberately does NOT skip past `(`: `assert!(source.contains("X"))`
    would otherwise walk back through the macro-call paren and read
    `assert!`'s own `!` as negating the `.contains(...)` inside it, which is
    exactly backwards -- that call asserts PRESENCE. A real negation always
    sits immediately (mod whitespace) before the expression it negates, with
    no intervening open-paren belonging to something else.
    """
    i = match_start - 1
    while i >= 0 and body[i] in " \t\n":
        i -= 1
    return i >= 0 and body[i] == "!"


def resolve_include_path(containing_file: pathlib.Path, literal: str) -> pathlib.Path:
    """`include_str!`'s argument is resolved relative to the file it appears
    in -- exactly like a Rust `mod` path, never relative to the crate root
    or the repository root."""
    return (containing_file.parent / literal).resolve()


class Pin:
    __slots__ = ("test_file", "test_line", "function", "kind", "needle", "target_file")

    def __init__(self, test_file, test_line, function, kind, needle, target_file):
        self.test_file = test_file
        self.test_line = test_line
        self.function = function
        self.kind = kind
        self.needle = needle
        self.target_file = target_file


def find_pins(path: pathlib.Path, source: str) -> tuple[list[Pin], list[str]]:
    """Pins found in one file, plus computed-needle notices (not failures).

    Scoped per enclosing FUNCTION, not per whole file: a variable name is
    tracked only within the function it is bound in, so two functions in the
    same file that each bind a local named `source` to a DIFFERENT
    `include_str!` target never cross-contaminate each other's needles.
    """
    pins: list[Pin] = []
    computed: list[str] = []
    seen_computed: set[tuple[int, str]] = set()

    fn_spans: list[tuple[int, int, str]] = []
    seen_fn_starts: set[int] = set()
    for match in FN_HEADER.finditer(source):
        start, end, name = enclosing_function_span(source, match.start())
        if start not in seen_fn_starts:
            seen_fn_starts.add(start)
            fn_spans.append((start, end, name))
    if not fn_spans:
        fn_spans = [(0, len(source), "<module scope>")]

    for start, end, fn_name in fn_spans:
        body = source[start:end]

        # Root bindings: name -> resolved target file, in the order they
        # appear (a name may be rebound; the LAST binding before a use wins,
        # same as Rust shadowing).
        roots: dict[str, pathlib.Path] = {}
        for inc_match in INCLUDE_BINDING.finditer(body):
            roots[inc_match.group("name")] = resolve_include_path(
                path, inc_match.group("path")
            )
        if not roots:
            continue

        # One derivation hop: a `let X = <rhs>;` whose RHS visibly names an
        # already-tracked identifier as a whole word extends tracking to X.
        # Applied in source order so a chain of two derivations in sequence
        # (rare in this tree, but not assumed absent) still resolves.
        tracked = dict(roots)
        for der_match in DERIVED_BINDING.finditer(body):
            name = der_match.group("name")
            if name in tracked:
                continue
            rhs = der_match.group("rhs")
            for base_name, target in tracked.items():
                if re.search(rf'\b{re.escape(base_name)}\b', rhs):
                    tracked[name] = target
                    break

        for needle_match in NEEDLE_CALL.finditer(body):
            var = needle_match.group("var")
            if var not in tracked:
                continue
            if _is_negated(body, needle_match.start()):
                # `!process_rdp.contains("NativeRdramRollback::new(")` (as in
                # `fn64-render-rt64/src/tests.rs`'s
                # `rt64_raw_rdp_submission_owns_context_and_invalidates_on_failure`)
                # asserts the needle is ABSENT. That is not a source pin this
                # lint's failure mode applies to: the needle disappearing is
                # the desired, asserted state, and a static grep has no way
                # to distinguish "it was deliberately removed" from "the pin
                # rotted" for a negative assertion the way it can for a
                # positive one. Skip it entirely -- not even as a computed
                # notice, since the needle IS a literal and there is nothing
                # ambiguous about it, just nothing this lint should gate.
                continue
            kind = needle_match.group("kind")
            needle = unescape(needle_match.group("needle"))
            abs_offset = start + needle_match.start()
            line = source.count("\n", 0, abs_offset) + 1
            pins.append(Pin(path, line, fn_name, kind, needle, tracked[var]))

        for needle_match in NEEDLE_CALL_COMPUTED.finditer(body):
            var = needle_match.group("var")
            if var not in tracked:
                continue
            arg = needle_match.group(3).strip()
            # Skip anything the literal-form regex already claimed: a plain
            # `"..."` argument matches NEEDLE_CALL_COMPUTED's `[^)]*` too
            # (its lookahead only excludes an IMMEDIATE opening quote), so a
            # literal call would otherwise be double-reported once as a pin
            # and once as "computed".
            if re.fullmatch(r'"(?:[^"\\]|\\.)*"', arg):
                continue
            abs_offset = start + needle_match.start()
            line = source.count("\n", 0, abs_offset) + 1
            key = (line, arg)
            if key in seen_computed:
                continue
            seen_computed.add(key)
            computed.append(
                f"{_display_path(path)}:{line}: .{needle_match.group('kind')}({arg}) "
                "is a computed needle -- not checkable by this lint, not gated"
            )
    return pins, computed


def tracked_rust_files() -> list[pathlib.Path]:
    import subprocess

    out = subprocess.run(
        ["git", "ls-files", "*.rs"], cwd=ROOT, capture_output=True, text=True, check=True
    ).stdout
    return [ROOT / p for p in out.split()]


def check_pin(pin: Pin) -> str | None:
    """None if the pin's needle still occurs in its target file; else a
    formatted failure naming the test site, the needle, and the target."""
    if not pin.target_file.is_file():
        rel_test = _display_path(pin.test_file)
        return (
            f"{rel_test}:{pin.test_line}: in {pin.function}(): include_str! target "
            f"{pin.target_file} does not exist"
        )
    text = pin.target_file.read_text(encoding="utf-8", errors="replace")
    if pin.needle in text:
        return None
    rel_test = _display_path(pin.test_file)
    rel_target = _display_path(pin.target_file)
    shown = pin.needle if len(pin.needle) <= 80 else pin.needle[:77] + "..."
    return (
        f"{rel_test}:{pin.test_line}: in {pin.function}(): .{pin.kind}(\"{shown}\") "
        f"-- needle no longer occurs in {rel_target}"
    )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="check the checker against synthetic sources, including a planted "
        "moved needle, and exit",
    )
    args = parser.parse_args(argv)
    if args.self_test:
        return self_test()

    failures: list[str] = []
    notices: list[str] = []
    pin_count = 0
    files_with_pins = 0
    for path in tracked_rust_files():
        source = path.read_text(encoding="utf-8", errors="replace")
        if "include_str!" not in source:
            continue
        pins, computed = find_pins(path, source)
        if pins:
            files_with_pins += 1
        pin_count += len(pins)
        notices.extend(computed)
        for pin in pins:
            failure = check_pin(pin)
            if failure:
                failures.append(failure)

    if notices:
        print(f"lint-source-pins: {len(notices)} computed needle(s), not checked:", file=sys.stderr)
        for notice in notices:
            print(f"  {notice}", file=sys.stderr)

    if failures:
        print(f"\n{len(failures)} source pin(s) broken:", file=sys.stderr)
        for failure in failures:
            print(f"  {failure}", file=sys.stderr)
        return 1

    print(
        f"lint-source-pins: clean ({pin_count} literal pin(s) across "
        f"{files_with_pins} file(s) all resolve)"
    )
    return 0


def self_test() -> int:
    """Exercise the checker against synthetic sources, including a planted
    moved needle -- the exact failure mode task 2.4 produced: a structural
    refactor moves text a pin's `.find`/`.contains` depended on, and the pin
    must fail NAMING the test and the needle, not go silently green."""
    import tempfile

    cases_passed = 0
    cases_failed: list[str] = []

    def case(label: str, fn) -> None:
        nonlocal cases_passed
        try:
            fn()
            cases_passed += 1
        except AssertionError as error:
            cases_failed.append(f"{label}: {error}")

    with tempfile.TemporaryDirectory(prefix="lint-source-pins-selftest-") as tmp:
        tmp_path = pathlib.Path(tmp)
        target = tmp_path / "lib.rs"
        target.write_text("fn process_task() {\n    do_the_thing();\n}\n")

        test_file = tmp_path / "tests.rs"
        test_file.write_text(
            'fn a_passing_pin() {\n'
            '    let source = include_str!("lib.rs");\n'
            '    assert!(source.contains("fn process_task"));\n'
            '    let start = source.find("do_the_thing").unwrap();\n'
            '}\n'
        )

        def passing_pin_is_clean():
            source = test_file.read_text()
            pins, computed = find_pins(test_file, source)
            assert len(pins) == 2, pins
            assert computed == [], computed
            for pin in pins:
                assert check_pin(pin) is None, check_pin(pin)

        case("a passing pin with two literal needles is clean", passing_pin_is_clean)

        # Plant the exact task-2.4 failure mode: rename the symbol in the
        # TARGET file so the test's needle no longer occurs anywhere in it.
        moved_file = tmp_path / "moved.rs"
        moved_file.write_text("fn handle_render_task() {\n    do_the_thing_else();\n}\n")
        moved_test = tmp_path / "moved_tests.rs"
        moved_test.write_text(
            'fn a_pin_whose_needle_moved() {\n'
            '    let source = include_str!("moved.rs");\n'
            '    assert!(source.contains("fn process_task"));\n'
            '}\n'
        )

        def planted_moved_needle_fails_and_names_it():
            source = moved_test.read_text()
            pins, _computed = find_pins(moved_test, source)
            assert len(pins) == 1, pins
            failure = check_pin(pins[0])
            assert failure is not None, "a moved needle must fail, not pass silently"
            assert "a_pin_whose_needle_moved" in failure, failure
            assert "fn process_task" in failure, failure
            assert str(moved_test.relative_to(tmp_path)) in failure or "moved_tests.rs" in failure

        case(
            "a planted moved needle is rejected, naming the test and the needle",
            planted_moved_needle_fails_and_names_it,
        )

        computed_test = tmp_path / "computed_tests.rs"
        computed_test.write_text(
            'fn a_computed_needle_is_reported_not_gated() {\n'
            '    let source = include_str!("lib.rs");\n'
            '    let needle = format!("fn {}", "process_task");\n'
            '    assert!(source.contains(&needle));\n'
            '}\n'
        )

        def computed_needle_is_reported_not_failed():
            source = computed_test.read_text()
            pins, computed = find_pins(computed_test, source)
            assert pins == [], pins
            assert len(computed) == 1, computed
            assert "computed" in computed[0]

        case(
            "a computed needle (format!) is reported, not checked or failed",
            computed_needle_is_reported_not_failed,
        )

        relative_test = tmp_path / "sub" / "tests.rs"
        relative_test.parent.mkdir()
        relative_test.write_text(
            'fn resolves_relative_to_the_macro_site() {\n'
            '    let source = include_str!("../lib.rs");\n'
            '    assert!(source.contains("fn process_task"));\n'
            '}\n'
        )

        def relative_include_path_resolves_beside_the_macro():
            source = relative_test.read_text()
            pins, _computed = find_pins(relative_test, source)
            assert len(pins) == 1, pins
            assert pins[0].target_file == target.resolve(), pins[0].target_file
            assert check_pin(pins[0]) is None

        case(
            "include_str! path resolves relative to the file containing the macro, "
            "not the repo root",
            relative_include_path_resolves_beside_the_macro,
        )

        missing_target_test = tmp_path / "missing_target_tests.rs"
        missing_target_test.write_text(
            'fn pins_against_a_file_that_does_not_exist() {\n'
            '    let source = include_str!("nope.rs");\n'
            '    assert!(source.contains("anything"));\n'
            '}\n'
        )

        def missing_target_fails_not_crashes():
            source = missing_target_test.read_text()
            pins, _computed = find_pins(missing_target_test, source)
            assert len(pins) == 1, pins
            failure = check_pin(pins[0])
            assert failure is not None
            assert "does not exist" in failure

        case(
            "an include_str! target that does not exist fails instead of crashing",
            missing_target_fails_not_crashes,
        )

    if cases_failed:
        for failure in cases_failed:
            print(f"SELF-TEST FAIL: {failure}", file=sys.stderr)
        print(f"{len(cases_failed)} self-test case(s) failed", file=sys.stderr)
        return 1
    print(f"lint-source-pins self-test: {cases_passed} cases passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
