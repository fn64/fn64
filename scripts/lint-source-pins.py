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
still occurs in the file's current text -- specifically, in the SAME BOUNDED
REGION the Rust code itself narrows to before checking the needle, not
merely somewhere in the whole file. This distinction is load-bearing for a
SAME-FILE pin (the `include_str!` names the very file the pinning test lives
in): a needle checked with an unbounded whole-file search is defeated by the
needle's own literal occurring elsewhere in that same file -- the pinning
test's own `.contains("...")` call, its assertion message, or (as with
`production.rs`'s module-level doc comment describing `publish_raw_dpc`) even
a PRODUCTION doc comment -- so mutating the real pinned code leaves the
needle "found" regardless. `production.rs`'s six pins and
`fn64-render-rt64/src/tests.rs`'s `method_source` helper all bound `source`
to one function's (or struct's) body via a `.find("fn NAME(")`-anchored
slice, a `.split_once(...)` pair, or a `.split(...).nth(N)` segment before
checking anything; this script re-derives that same bound against the LIVE
target text (see `resolve_bound_regions`, `_eval_offset_expr`,
`_eval_region_expr`, `_eval_split_chain`, `_eval_method_source_call`) so the
needle is checked in exactly the region the Rust assertion actually
inspects. A needle built from a variable, `format!`, or `concat!` cannot be
checked this way -- those are reported (not failed) as "computed, not
checked" so the gap is visible rather than silently swallowed. A bounding
expression this script cannot confidently re-derive falls back to the whole
resolved file (less precise, never silently narrower than what the Rust
code could have meant) -- currently zero same-file pins in this tree hit
that fallback; a handful of cross-file pins in a frozen `evidence/` snapshot
predating the `method_source` helper still do, safely.

Usage: scripts/lint-source-pins.py           (exit 0 clean, 1 on any broken pin)
       scripts/lint-source-pins.py --self-test

ponytail: one regex pass per test file, no AST, no full data-flow -- but
enough name tracking, and enough of a small region-bound evaluator, to
matter. A function that pins two different `include_str!` targets
(`fn64-render-rt64/src/ffi/tests.rs` pins `fn64_rt64_shim.cpp`,
`CMakeLists.txt`, and three more headers in ONE test) must not credit a
needle checked against `cmake` as though it were checked against `shim` --
so each `let VAR = include_str!(...)` binding is tracked by NAME,
`VAR.find(...)`/`VAR.contains(...)` calls are matched to the binding they
were actually called on, and the bound-region evaluator is N-hop transitive
within one function (not a fixed "one further hop": `resolve_bound_regions`
re-checks the whole growing `values` dict against each further `let`
statement in source order, so a chain of two, three, or more derivations in
sequence still resolves). A needle called on a name this script cannot trace
back to an `include_str!` binding at all is not a pin and is silently
skipped, not reported -- most `.find`/`.contains` calls in the tree have
nothing to do with a source pin.
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
    __slots__ = (
        "test_file", "test_line", "function", "kind", "needle", "target_file", "region",
    )

    def __init__(self, test_file, test_line, function, kind, needle, target_file, region=None):
        self.test_file = test_file
        self.test_line = test_line
        self.function = function
        self.kind = kind
        self.needle = needle
        self.target_file = target_file
        # The bounded region (offsets into the LIVE target text) the Rust
        # code actually checks this needle against, if this script could
        # re-derive it; `None` when it couldn't (falls back to the whole
        # file in `check_pin`, same as before the bounded-region fix).
        self.region = region


class Region:
    """A byte-offset span `[start, end)` into a target file's LIVE text --
    what a Rust `&str` slice binding (`&source[a..b]`) actually denotes at
    lint time, re-derived the same way the Rust code derives it at test
    time. `start`/`end` are `None` when this script cannot confidently
    re-derive them (an expression shape it doesn't recognize); a needle
    checked against such a region falls back to the WHOLE resolved target
    text, same as before this fix -- less precise, never silently narrower
    than what the Rust code could have meant.
    """

    __slots__ = ("start", "end")

    def __init__(self, start: int | None, end: int | None):
        self.start = start
        self.end = end

    def whole(self) -> "Region":
        return self

    @staticmethod
    def full(text: str) -> "Region":
        return Region(0, len(text))

    def unresolved(self) -> bool:
        return self.start is None or self.end is None

    def slice_of(self, text: str) -> str:
        if self.unresolved():
            return text
        return text[self.start:self.end]


# --- bounded-region resolution ------------------------------------------
#
# A same-file pin (`include_str!` of the very file the pinning test lives
# in) cannot be checked by asking "does the needle occur anywhere in the
# file" -- the reviewer proved this by mutating `production.rs`'s real
# `publish_raw_dpc` body while leaving its OWN module-level doc comment
# (line 14: "...publishes through exactly
# `self.coordinator.prepare_publication(publication).commit()`.") and the
# pinning test's own `.contains("...")` literal and assertion message
# untouched; the needle survives in the file regardless of what happens to
# the real code, because it is quoted verbatim in THREE other places in the
# same file. Stripping `#[cfg(test)]` regions (this lint's first attempt)
# does not fix this: the doc-comment occurrence is in PRODUCTION text.
#
# The correct fix is to check the needle only in the same bounded region
# the Rust test itself computes before calling `.find`/`.contains` on it --
# `production.rs`'s six pins and `fn64-render-rt64/src/tests.rs`'s
# `method_source` helper all narrow `source` to one function's (or one
# struct's) body via a `.find("fn NAME(")`-anchored slice before checking
# anything. This section re-derives that SAME slice against the live target
# text, so the doc comment at line 14 and the test's own literal at line
# 12983 are simply outside the region being searched, exactly as they are
# outside the region the Rust `body.contains(...)` call actually searches.
#
# Supported shapes (verified against every derived binding in this tree):
#   let X = BASE.find("literal").expect(...);          -- an offset, in BASE
#   let X = BASE[A..].find("literal")....;              -- an offset, in BASE sliced from A
#   let X = &BASE[A..B];  / BASE[A..B]                   -- a bounded region
#   let X = &BASE[A..];   / BASE[A..]                    -- a region to EOF
#   let X = &BASE[..B];                                  -- a region from 0
# combined with a trailing `+ IDENT` / `+ INT` adjustment and an optional
# `.map(|o| EXPR).unwrap_or(EXPR)` / `.map_or(EXPR, |end| EXPR)` wrapper
# around a `.find(...)` result -- both of which this tree uses to fold a
# RELATIVE offset (from a sliced search) back into an ABSOLUTE one.
#
# Anything outside this shape set resolves to `Region(None, None)` --
# unresolved, not wrong: `check_pin` falls back to the whole file for it,
# same behavior as before this fix, rather than guessing a bound that could
# silently exclude a real occurrence.

_INT = r'\d+'
_IDENT = r'[A-Za-z_][A-Za-z0-9_]*'

# `IDENT.find("literal")` possibly preceded by a slice (`IDENT[A..]`), used
# both as a standalone offset expression and as the search step inside a
# `.map(...)`/`.map_or(...)` chain.
_FIND_LITERAL = re.compile(
    r'^(?P<base>' + _IDENT + r')(?:\[(?P<slice_from>[^\]]*)\.\.\])?'
    r'\s*\.\s*find\(\s*"(?P<needle>(?:[^"\\]|\\.)*)"\s*\)'
)

# A trailing `+ IDENT` or `+ INT` adjustment, e.g. `... + body_start`.
_PLUS_TAIL = re.compile(r'^\s*\+\s*(?P<term>' + _IDENT + r'|' + _INT + r')\s*$')


def _offsets(name: str, values: dict) -> int | None:
    """Look up a previously-resolved plain integer offset by name, or parse
    a bare integer literal. `None` if neither."""
    if name in values and isinstance(values[name], int):
        return values[name]
    if re.fullmatch(_INT, name):
        return int(name)
    return None


def _eval_offset_expr(expr: str, target_text: str, values: dict) -> int | None:
    """Evaluate an offset-producing RHS (something used as a slice bound or
    added to one) against `target_text`, using already-resolved bindings in
    `values`. Returns `None` if the shape isn't recognized.

    `values` maps name -> either an `int` (a resolved offset) or a `Region`
    (a resolved slice) -- both occur as bases: `body_start + 1` needs the
    former, `source[body_start..]` needs the latter for `source` itself
    (always the full-file root, so always a `Region`).
    """
    expr = expr.strip()

    # Bare identifier or integer literal.
    plain = _offsets(expr, values)
    if plain is not None:
        return plain

    # `BASE.method_chain` possibly ending in `.expect(...)`/`.unwrap()`, or
    # wrapped in `.map(|o| TAIL).unwrap_or(FALLBACK)` /
    # `.map_or(FALLBACK, |o| TAIL)` -- and, in this tree, sometimes ALSO
    # followed by a trailing `+ IDENT` AFTER the `.expect(...)`
    # (`source[body_start..].find("...").expect("...") + body_start`, the
    # exact shape `body_end` uses). `.expect(...)`/`.unwrap()` therefore
    # cannot be stripped only when anchored at the string's end -- it must
    # be stripped WHEREVER it appears right after the find, and whatever
    # follows it (end-of-string, `.map...`, or `+ IDENT`) is handled by the
    # `rest`-dispatch below, not consumed here.
    stripped = expr

    # `BASE[FROM..].find("literal")` (optionally chained further below) --
    # match the FIND at the front, then handle what follows it.
    find_match = _FIND_LITERAL.match(stripped)
    if find_match:
        base_name = find_match.group("base")
        base_value = values.get(base_name)
        if isinstance(base_value, Region) and not base_value.unresolved():
            base_text = target_text[base_value.start:base_value.end]
            base_offset = base_value.start
        elif base_name == "source" or base_name not in values:
            # The include_str! root itself, or an unseen base: treat as the
            # whole target text (offset 0). Every base in this tree is
            # either the root `source`/`tlut`/`shim`/etc. binding (a
            # Region covering the whole file) or a `Region`-typed derived
            # slice already handled above.
            base_text = target_text
            base_offset = 0
        else:
            return None

        slice_from_expr = find_match.group("slice_from")
        if slice_from_expr:
            slice_from = _eval_offset_expr(slice_from_expr, target_text, values)
            if slice_from is None:
                return None
            base_text = base_text[slice_from:]
            base_offset += slice_from

        pos = base_text.find(unescape(find_match.group("needle")))
        if pos == -1:
            # The anchor itself doesn't exist any more -- the region cannot
            # be resolved. This is itself evidence of a broken pin, but the
            # anchor is not a needle THIS lint checks (the brief scopes it
            # to find/contains needle checks, not to auxiliary `.find`
            # anchors); leaving it unresolved makes check_pin fall back to
            # the whole file rather than mis-reporting an unrelated needle.
            return None
        relative_offset = pos
        rest = stripped[find_match.end():]
        # `.expect("...")`/`.unwrap()` right after the find, wherever it
        # sits in the chain (NOT anchored to end-of-string, since a `+
        # IDENT` term can follow it -- `body_end`'s
        # `.find(...).expect(...) + body_start` is exactly this shape).
        # Neither call changes the produced offset on the success path this
        # static check assumes.
        rest = re.sub(r'^\s*\.\s*expect\(\s*"(?:[^"\\]|\\.)*"\s*\)', '', rest, count=1)
        rest = re.sub(r'^\s*\.\s*unwrap\(\)', '', rest, count=1)
        # `.unwrap_or_else(|| panic!(...))` -- `method_source`'s shape. A
        # panic-on-failure fallback closure is, for this static check's
        # purposes, the same as `.expect`/`.unwrap`: it does not change the
        # produced offset on the success path.
        rest = re.sub(
            r'^\s*\.\s*unwrap_or_else\(\s*\|\|\s*panic!\([^)]*\)\s*\)', '', rest, count=1
        )

        # `.map(|NAME| TAIL).unwrap_or(FALLBACK)` -- TAIL is evaluated with
        # NAME bound to the raw (relative) find() result.
        map_unwrap = re.match(
            r'^\s*\.\s*map\(\s*\|(?P<var>' + _IDENT + r')\|\s*(?P<tail>.*?)\)'
            r'\s*\.\s*unwrap_or\(\s*(?P<fallback>.*)\)\s*$',
            rest,
        )
        if map_unwrap:
            inner_values = dict(values)
            inner_values[map_unwrap.group("var")] = relative_offset
            return _eval_offset_expr(map_unwrap.group("tail"), target_text, inner_values)

        # `.map_or(FALLBACK, |NAME| TAIL)` -- the other argument order this
        # tree uses (`method_source`'s `impl_end`).
        map_or = re.match(
            r'^\s*\.\s*map_or\(\s*(?P<fallback>.*?),\s*\|(?P<var>' + _IDENT + r')\|\s*(?P<tail>.*)\)\s*$',
            rest,
        )
        if map_or:
            inner_values = dict(values)
            inner_values[map_or.group("var")] = relative_offset
            return _eval_offset_expr(map_or.group("tail"), target_text, inner_values)

        if rest.strip() == "":
            return base_offset + relative_offset

        # A trailing `+ IDENT`/`+ INT` after the bare find (not the
        # map/map_or forms above): `body_start + 1` style additions are
        # handled by the caller composing this function's result, but a
        # find's own result plus a literal integer offset (rare, not
        # currently in the tree) is still resolvable here.
        plus = _PLUS_TAIL.match(rest)
        if plus:
            term = _offsets(plus.group("term"), values)
            if term is None:
                return None
            return base_offset + relative_offset + term

        return None

    # `TERM + REST` (e.g. `body_start + 1`, or the further term in a chain
    # like `body_start + 1 + offset`, where the LHS after the first split
    # is the bare integer literal `1`) -- LHS is either an identifier
    # already resolved to a plain offset, or an integer literal; REST
    # recurses so a three-(or more-)term sum resolves left-to-right.
    plus_expr = re.match(
        r'^(?P<lhs>' + _IDENT + r'|' + _INT + r')\s*\+\s*(?P<rhs>.+)$', stripped
    )
    if plus_expr:
        lhs = _offsets(plus_expr.group("lhs"), values)
        rhs = _eval_offset_expr(plus_expr.group("rhs"), target_text, values)
        if lhs is None or rhs is None:
            return None
        return lhs + rhs

    if re.fullmatch(_IDENT + r'\s*\.\s*len\(\)', stripped):
        base_name = stripped.split(".")[0].strip()
        base_value = values.get(base_name)
        if isinstance(base_value, Region) and not base_value.unresolved():
            return base_value.end - base_value.start
        if base_name == "source" or base_name not in values:
            return len(target_text)

    return None


def _base_region(base_name: str, target_text: str, values: dict) -> tuple[int, int] | None:
    """`(start, end)` byte offsets of `base_name`'s current binding into
    `target_text` -- the whole target if `base_name` is the include_str!
    root (or an as-yet-unseen name, which in this tree only ever means the
    root), or a previously resolved `Region`'s span. `None` if `base_name`
    is a KNOWN binding of some other, unresolved shape (do not silently
    treat it as the whole file in that case)."""
    base_value = values.get(base_name)
    if isinstance(base_value, Region):
        if base_value.unresolved():
            return None
        return base_value.start, base_value.end
    if base_name == "source" or base_name not in values:
        return 0, len(target_text)
    return None


def _eval_split_chain(expr: str, target_text: str, values: dict) -> "Region | None":
    """`BASE.split_once("A").expect(...).1.split_once("B").expect(...).0`
    and `BASE.split("A").nth(N).and_then(|tail| tail.split("B").next())` --
    the two split-based bounding shapes this tree uses
    (`load_tlut.rs:1334`, `raw_dpc/production_adapter.rs:1449`). Neither is
    a `.find()`-anchored slice, so `_eval_offset_expr`/the plain slice
    matcher in `_eval_region_expr` never see them; handled here as their
    own small chain-walker over the SAME text/offset bookkeeping.

    Returns the resulting `Region`, or `None` if the shape doesn't match at
    all (not a split chain) or a step chokes (a needle no longer found,
    an index out of range) -- both fall through to "unresolved," which
    `check_pin` treats as evidence-worthy the same as any other broken
    anchor.
    """
    stripped = expr.strip()
    base_match = re.match(r'^(?P<base>' + _IDENT + r')\s*\.\s*split', stripped)
    if not base_match:
        return None
    base_name = base_match.group("base")
    base_span = _base_region(base_name, target_text, values)
    if base_span is None:
        return None
    start, end = base_span
    rest = stripped[len(base_name):]

    while rest.strip():
        # `.split_once("literal")` -> a 2-tuple region pair, this call's
        # own two halves recorded as (before, after) spans; the NEXT
        # `.0`/`.1`/`.expect(...)` selects which one survives.
        split_once = re.match(
            r'^\s*\.\s*split_once\(\s*"(?P<needle>(?:[^"\\]|\\.)*)"\s*\)', rest
        )
        if split_once:
            needle = unescape(split_once.group("needle"))
            text = target_text[start:end]
            pos = text.find(needle)
            if pos == -1:
                return None
            before = (start, start + pos)
            after = (start + pos + len(needle), end)
            rest = rest[split_once.end():]
            rest = re.sub(r'^\s*\.\s*expect\(\s*"(?:[^"\\]|\\.)*"\s*\)', '', rest, count=1)
            rest = re.sub(r'^\s*\.\s*unwrap\(\)', '', rest, count=1)
            select = re.match(r'^\s*\.\s*(?P<idx>[01])\b', rest)
            if not select:
                return None
            start, end = before if select.group("idx") == "0" else after
            rest = rest[select.end():]
            continue

        # `.split("literal").nth(N)` -> the Nth (0-indexed) segment;
        # optionally followed by `.and_then(|tail| tail.split("literal2")
        # .next())`, which narrows further to the first sub-segment of a
        # SECOND split applied to that Nth segment.
        split_nth = re.match(
            r'^\s*\.\s*split\(\s*"(?P<needle>(?:[^"\\]|\\.)*)"\s*\)'
            r'\s*\.\s*nth\(\s*(?P<n>\d+)\s*\)', rest
        )
        if split_nth:
            needle = unescape(split_nth.group("needle"))
            n = int(split_nth.group("n"))
            segments = target_text[start:end].split(needle)
            if n >= len(segments):
                return None
            offset = start + sum(len(s) + len(needle) for s in segments[:n])
            seg_start, seg_end = offset, offset + len(segments[n])
            rest = rest[split_nth.end():]
            rest = re.sub(r'^\s*\.\s*expect\(\s*"(?:[^"\\]|\\.)*"\s*\)', '', rest, count=1)

            and_then = re.match(
                r'^\s*\.\s*and_then\(\s*\|(?P<var>' + _IDENT + r')\|\s*'
                r'(?P=var)\s*\.\s*split\(\s*"(?P<needle2>(?:[^"\\]|\\.)*)"\s*\)'
                r'\s*\.\s*next\(\)\s*\)', rest
            )
            if and_then:
                needle2 = unescape(and_then.group("needle2"))
                text2 = target_text[seg_start:seg_end]
                cut = text2.find(needle2)
                seg_end = seg_start + cut if cut != -1 else seg_end
                rest = rest[and_then.end():]
            start, end = seg_start, seg_end
            rest = re.sub(r'^\s*\.\s*expect\(\s*"(?:[^"\\]|\\.)*"\s*\)', '', rest, count=1)
            continue

        return None

    return Region(start, end)


def _eval_region_expr(expr: str, target_text: str, values: dict) -> "Region | None":
    """Evaluate an RHS that produces a `Region` -- a `&BASE[A..B]` slice (in
    any of the `A..B` / `A..` / `..B` forms), a `.split`/`.split_once`
    chain, or a bare identifier already bound to a `Region`. `None` if the
    shape isn't recognized (the caller treats that as "not a
    region-producing binding," not as an error)."""
    expr = expr.strip().lstrip("&").strip()

    if expr in values and isinstance(values[expr], Region):
        return values[expr]

    split_region = _eval_split_chain(expr, target_text, values)
    if split_region is not None:
        return split_region

    slice_match = re.match(
        r'^(?P<base>' + _IDENT + r')\s*\[\s*(?P<lo>[^\]]*?)\.\.(?P<hi>[^\]]*?)\s*\]$',
        expr,
    )
    if not slice_match:
        return None

    base_name = slice_match.group("base")
    base_value = values.get(base_name)
    if isinstance(base_value, Region):
        if base_value.unresolved():
            return Region(None, None)
        base_text_start = base_value.start
        base_len = base_value.end - base_value.start
    elif base_name == "source" or base_name not in values:
        base_text_start = 0
        base_len = len(target_text)
    else:
        return None

    lo_expr = slice_match.group("lo").strip()
    hi_expr = slice_match.group("hi").strip()
    lo = 0 if lo_expr == "" else _eval_offset_expr(lo_expr, target_text, values)
    hi = base_len if hi_expr == "" else _eval_offset_expr(hi_expr, target_text, values)
    if lo is None or hi is None:
        return Region(None, None)
    return Region(base_text_start + lo, base_text_start + hi)


# `method_source(BASE, "impl header literal", "method name literal")` --
# `fn64-render-rt64/src/tests.rs`'s bounded helper, matched here so this
# lint follows its EXACT documented algorithm rather than falling back to
# the whole file for the pins that use it. Both string arguments must be
# literals for this to apply; a computed impl_header/method (none exist in
# this tree today) falls through unresolved, same as any other unrecognized
# shape.
METHOD_SOURCE_CALL = re.compile(
    r'^method_source\(\s*(?P<base>' + _IDENT + r')\s*,\s*'
    r'"(?P<impl_header>(?:[^"\\]|\\.)*)"\s*,\s*'
    r'"(?P<method>(?:[^"\\]|\\.)*)"\s*,?\s*\)$',
    re.DOTALL,
)


def _eval_method_source_call(rhs: str, target_text: str, values: dict) -> "Region | None":
    """Replicate `fn64-render-rt64/src/tests.rs`'s `method_source` helper
    exactly (see its own doc comment): find `impl_header` in `BASE`'s text,
    bound that impl block at its own closing `\\n}` (or EOF), find
    `    fn METHOD(` inside the block, and bound the method body at the
    next `\\n    fn ` sibling (or the block's end). Returns a `Region` into
    `target_text`, or `None` if the call shape doesn't match or either
    literal can no longer be found (an unresolvable region, not a needle
    failure this lint reports directly)."""
    call_match = METHOD_SOURCE_CALL.match(rhs.strip())
    if not call_match:
        return None
    base_name = call_match.group("base")
    base_value = values.get(base_name)
    if isinstance(base_value, Region) and not base_value.unresolved():
        base_start, base_end = base_value.start, base_value.end
    elif base_name == "source" or base_name not in values:
        base_start, base_end = 0, len(target_text)
    else:
        return None
    base_text = target_text[base_start:base_end]

    impl_header = unescape(call_match.group("impl_header"))
    impl_start = base_text.find(impl_header)
    if impl_start == -1:
        return None
    rest = base_text[impl_start:]
    close = rest.find("\n}")
    impl_end = len(rest) if close == -1 else close + 1
    block = rest[:impl_end]

    method = unescape(call_match.group("method"))
    needle = f"    fn {method}("
    method_start = block.find(needle)
    if method_start == -1:
        return None
    after = block[method_start + len(needle):]
    next_fn = after.find("\n    fn ")
    method_end = len(block) if next_fn == -1 else method_start + len(needle) + next_fn

    absolute_start = base_start + impl_start + method_start
    absolute_end = base_start + impl_start + method_end
    return Region(absolute_start, absolute_end)


def resolve_bound_regions(
    body: str, roots: dict, target_text_by_root: dict
) -> dict:
    """Extend `roots` (name -> Region, the include_str! bindings, each
    already `Region.full(target_text)`) with every derived binding this
    function body computes, evaluated against the LIVE text of whichever
    target file that root binding resolves to.

    Returns a dict name -> Region | int (an int when the binding is a plain
    offset rather than a slice, e.g. `body_start`), covering everything
    `_eval_offset_expr`/`_eval_region_expr` could resolve. A binding this
    resolver doesn't understand is simply absent from the result; callers
    fall back to the FULL target text for any needle checked against it
    (via `tracked`'s separate, path-only dict, which is populated
    regardless of whether a Region could be derived).
    """
    values: dict = dict(roots)
    for der_match in DERIVED_BINDING.finditer(body):
        name = der_match.group("name")
        if name in values:
            continue
        rhs = der_match.group("rhs")
        # Which target text applies depends on which root this expression's
        # bases eventually derive from. Since every base name in an
        # expression must already be in `values` (or be the literal
        # identifier `source`, which is the common root name in this
        # tree), find ANY already-tracked root Region reachable from the
        # bases mentioned in `rhs` and use its target text. If none is
        # found, fall back to the first known root -- correct whenever a
        # function pins only one target (true for every case in this tree
        # today).
        target_text = None
        for root_name, root_region in roots.items():
            if re.search(rf'\b{re.escape(root_name)}\b', rhs) or len(roots) == 1:
                target_text = target_text_by_root.get(root_name)
                break
        if target_text is None:
            continue

        method_source_region = _eval_method_source_call(rhs, target_text, values)
        if method_source_region is not None:
            values[name] = method_source_region
            continue
        region = _eval_region_expr(rhs, target_text, values)
        if region is not None:
            values[name] = region
            continue
        offset = _eval_offset_expr(rhs, target_text, values)
        if offset is not None:
            values[name] = offset
    return values


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
        # (rare in this tree, but not assumed absent) still resolves. This
        # is the FILE-level tracking (which target a name derives from);
        # `region_values` below separately tracks the REGION within that
        # file, which is a strictly harder, best-effort computation that
        # can fail without losing file-level attribution.
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

        # Region-level tracking: for every root, load its live target text
        # once and seed a `Region.full(...)` binding; then re-derive every
        # bounded slice (`let body = &source[a..b];` and friends) the same
        # way the Rust code computes it. A binding this can't resolve is
        # simply absent, and `check_pin` falls back to the whole file for
        # any needle checked against it.
        target_text_by_root: dict[str, str] = {}
        region_roots: dict[str, Region] = {}
        for root_name, target_file in roots.items():
            if target_file.is_file():
                text = target_file.read_text(encoding="utf-8", errors="replace")
                target_text_by_root[root_name] = text
                region_roots[root_name] = Region.full(text)
        region_values = resolve_bound_regions(body, region_roots, target_text_by_root)

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
            region_value = region_values.get(var)
            region = region_value if isinstance(region_value, Region) else None
            pins.append(Pin(path, line, fn_name, kind, needle, tracked[var], region))

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
    """None if the pin's needle still occurs in the pinned REGION of its
    target file; else a formatted failure naming the test site, the needle,
    and the target.

    The needle is checked against the SAME bounded slice the Rust test
    itself computes before calling `.find`/`.contains` on it, when this
    script could re-derive that slice (`pin.region`, from
    `resolve_bound_regions`) -- `production.rs`'s six pins and
    `fn64-render-rt64/src/tests.rs`'s `method_source` helper all narrow
    `source` to one function's (or struct's) body via a
    `.find("fn NAME(")`-anchored slice first. Checking the WHOLE target
    file instead of that region is unsound for a SAME-FILE pin specifically
    (the `include_str!` names the very file the pinning test lives in):
    the needle is quoted verbatim elsewhere in that same file -- the
    pinning test's own `.contains("...")`/`.find("...")` call literal, its
    doc comment, its assertion failure message, and sometimes (as with
    `production.rs`'s module-level `//!` doc comment describing
    `publish_raw_dpc`) even a PRODUCTION doc comment -- so a whole-file
    search finds the needle regardless of what happens to the real pinned
    code and never fails. Reviewer-verified: mutating `production.rs`'s
    real `publish_raw_dpc` body (replacing `.commit()` with `.finish_up()`)
    left an unbounded whole-file search green, because the identical
    needle string survives at the file's own line 14 doc comment and the
    pinning test's own source a few hundred lines later.

    When `pin.region` is `None` (this script could not confidently
    re-derive the Rust code's slicing -- an unrecognized expression shape),
    this falls back to the whole resolved target file, same as before the
    bounded-region fix -- less precise, but never silently narrower than
    what the Rust code could have meant.
    """
    if not pin.target_file.is_file():
        rel_test = _display_path(pin.test_file)
        return (
            f"{rel_test}:{pin.test_line}: in {pin.function}(): include_str! target "
            f"{pin.target_file} does not exist"
        )
    text = pin.target_file.read_text(encoding="utf-8", errors="replace")
    if pin.region is not None and not pin.region.unresolved():
        searched = pin.region.slice_of(text)
        bounded = True
    else:
        searched = text
        bounded = False
    if pin.needle in searched:
        return None
    rel_test = _display_path(pin.test_file)
    rel_target = _display_path(pin.target_file)
    shown = pin.needle if len(pin.needle) <= 80 else pin.needle[:77] + "..."
    region_note = " (within the pinned region, not the whole file)" if bounded else ""
    return (
        f"{rel_test}:{pin.test_line}: in {pin.function}(): .{pin.kind}(\"{shown}\") "
        f"-- needle no longer occurs in {rel_target}{region_note}"
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

        # The reviewer's exact reproduction: a SAME-FILE pin (the
        # `include_str!` names the file the pinning test itself lives in)
        # whose target text quotes the needle in a doc comment / the test's
        # own assertion literal ELSEWHERE in the file, so a naive
        # whole-file search stays green no matter what happens to the real
        # pinned code. Bounded by a `.find("fn NAME(")`-anchored slice
        # (`production.rs`'s exact shape), the pin must fail once that
        # slice's own content changes -- regardless of what the rest of
        # the file still says.
        same_file_target = tmp_path / "same_file.rs"
        same_file_target.write_text(
            '//! Module doc: this file publishes through exactly\n'
            '//! `self.commit_real()`.\n'
            '\n'
            'fn publish() {\n'
            '    self.commit_real();\n'
            '}\n'
            '\n'
            '#[cfg(test)]\n'
            'mod tests {\n'
            '    #[test]\n'
            '    fn publish_is_exactly_commit_real() {\n'
            '        let source = include_str!("same_file.rs");\n'
            '        let body_start = source\n'
            '            .find("fn publish(")\n'
            '            .expect("publish must exist");\n'
            '        let body_end = source[body_start..]\n'
            '            .find("\\n}\\n")\n'
            '            .expect("publish must have a closing brace")\n'
            '            + body_start;\n'
            '        let body = &source[body_start..body_end];\n'
            '        assert!(\n'
            '            body.contains("self.commit_real()"),\n'
            '            "publish must call self.commit_real() -- see \\\n'
            '             `self.commit_real()`"\n'
            '        );\n'
            '    }\n'
            '}\n'
        )

        def same_file_pin_resolves_a_bounded_region_not_the_whole_file():
            source = same_file_target.read_text()
            pins, _computed = find_pins(same_file_target, source)
            body_pins = [p for p in pins if p.needle == "self.commit_real()"]
            assert len(body_pins) == 1, pins
            for pin in body_pins:
                assert pin.region is not None and not pin.region.unresolved(), (
                    "a same-file `&source[body_start..body_end]` pin must "
                    "resolve a bounded region, not fall back to the whole file"
                )
                assert check_pin(pin) is None

            # Mutate ONLY the real pinned function body -- leave the
            # module doc comment and the test's own literal/assertion
            # message untouched, exactly as the reviewer's probe did to
            # production.rs. A whole-file search would still find the
            # needle (in the doc comment and the test's own source); a
            # bounded-region search must not.
            mutated = source.replace(
                "fn publish() {\n    self.commit_real();\n}",
                "fn publish() {\n    self.commit_fake();\n}",
            )
            assert mutated != source
            mutated_pins, _c = find_pins(same_file_target, mutated)
            mutated_body_pins = [p for p in mutated_pins if p.needle == "self.commit_real()"]
            assert len(mutated_body_pins) == 1, mutated_pins
            for pin in mutated_body_pins:
                # check_pin reads the file from disk, so write the mutation
                # there for this half of the assertion.
                same_file_target.write_text(mutated)
                try:
                    failure = check_pin(pin)
                finally:
                    same_file_target.write_text(source)
                assert failure is not None, (
                    "a same-file pin whose real body changed must fail even "
                    "though the needle survives elsewhere in the file (the "
                    "module doc comment, the test's own assertion literal) "
                    "-- this is the reviewer's exact reproduction"
                )
                assert "publish_is_exactly_commit_real" in failure, failure
                assert "self.commit_real()" in failure, failure

        case(
            "a same-file pin (the reviewer's exact reproduction) fails when the "
            "REAL pinned body changes, even though the needle survives in the "
            "file's own doc comment and the test's own assertion literal",
            same_file_pin_resolves_a_bounded_region_not_the_whole_file,
        )

        def doc_comment_only_occurrence_does_not_satisfy_the_pin():
            # A minimal, more surgical version of the same claim: a needle
            # that occurs ONLY in a doc comment above the bounded function
            # (never inside the function body itself) must not satisfy a
            # pin whose region is that function's body.
            doc_only_target = tmp_path / "doc_only.rs"
            doc_only_target.write_text(
                '/// mentions `sentinel_call()` here, in the doc comment only.\n'
                'fn traced() {\n'
                '    other_call();\n'
                '}\n'
            )
            doc_only_test = tmp_path / "doc_only_tests.rs"
            doc_only_test.write_text(
                'fn body_must_contain_sentinel_call() {\n'
                '    let source = include_str!("doc_only.rs");\n'
                '    let body_start = source.find("fn traced(").expect("exists");\n'
                '    let body_end = source[body_start..].find("\\n}\\n").expect("closes") + body_start;\n'
                '    let body = &source[body_start..body_end];\n'
                '    assert!(body.contains("sentinel_call()"));\n'
                '}\n'
            )
            source = doc_only_test.read_text()
            pins, _computed = find_pins(doc_only_test, source)
            body_pins = [p for p in pins if p.needle == "sentinel_call()"]
            assert len(body_pins) == 1, pins
            pin = body_pins[0]
            assert pin.region is not None and not pin.region.unresolved(), (
                "the bounded region must resolve so the doc-comment "
                "occurrence can be correctly excluded"
            )
            failure = check_pin(pin)
            assert failure is not None, (
                "a needle present only in a doc comment ABOVE the bounded "
                "function must not satisfy a pin scoped to that function's body"
            )
            assert "sentinel_call" in failure, failure

        case(
            "a doc-comment-only occurrence (outside the pinned region) does not "
            "satisfy the pin",
            doc_comment_only_occurrence_does_not_satisfy_the_pin,
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
