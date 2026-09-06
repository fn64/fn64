#!/usr/bin/env python3
"""Reject direct process-environment reads from the runtime library crates.

# What changed in task 2.2b, and why

This lint used to allowlist FUNCTIONS: a hand-maintained map of ten
`file -> (function, ...)` entries, each of which was checked for
`env::var`/`var_os` in its body. That is the wrong denominator. It could only
ever catch a regression in a function someone had already remembered to
register, so the 126 direct reads elsewhere in the same five crates were
invisible to it -- including every read the lint's own docstring was written
to discourage. A registry that covers 8% of its subject reports "clean" for
the other 92%.

It now allowlists CALL SITES per crate: in each crate below, the only
permitted `env::var`/`var_os` call site is inside that crate's `diag_env`
module (`diag_env` / `diag_env_present`), which is the single seam every
`diagnostic`-class knob reads through. `user`-class knobs do not appear here
at all -- they are resolved once by `fn64-shell`'s typed `Knobs` (flag >
`fn64.toml` > `FN64_*` compat > default) and passed in as values.

So the rule is now a property of the crate, not of a list: adding a new direct
read anywhere in these crates fails, whether or not anyone remembers this
file exists.

# Scope

Library sources only (`crates/<crate>/src`, excluding `src/bin`). A binary
crate root cannot reach its library's private `diag_env` module, and a
standalone diagnostic tool is not the runtime path this lint protects.
Test modules ARE in scope: a test that reads the environment directly is
reaching around the typed config the same way production code would, and in a
shared-process test binary it is also mutating global state under its
neighbors.

Run with `--self-test` to exercise the checker against synthetic sources,
including a planted direct read that must be rejected.
"""

from __future__ import annotations

import argparse
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]

# The crates whose library sources may not read the environment directly.
# Every one of these was migrated in task 2.2b; the count in the comment is
# the number of direct call sites that existed before it.
CRATES = (
    "fn64-render",  # 0 before: already clean, listed so it cannot regress
    "fn64-render-wgpu",  # 33 before
    "fn64-abi",  # 74 before
    "fn64-runtime",  # 6 before
    "fn64-audio",  # 17 before (library; src/bin is out of scope)
)

# The functions a permitted call site may live in. These are `diag_env.rs`'s
# own two entry points -- the seam every diagnostic knob reads through.
PERMITTED_FUNCTIONS = ("diag_env", "diag_env_present")

# The file a permitted call site may live in, relative to `<crate>/src`.
PERMITTED_FILE = "diag_env.rs"

ENV_READ = re.compile(r"(?:std::)?env::(?:var|var_os)\s*\(")

FUNCTION = re.compile(
    r"(?m)^(?P<indent>[ \t]*)(?:pub(?:\([^)]*\))?\s+)?(?:unsafe\s+)?"
    r"(?:extern\s+\"C\"\s+)?fn\s+(?P<name>[A-Za-z0-9_]+)\b"
)


def enclosing_function(source: str, offset: int) -> str | None:
    """Name of the innermost `fn` whose header precedes `offset`.

    Approximate by design: it is used only to check that a read inside
    `diag_env.rs` is in one of the two permitted functions, and that file is
    small and flat. Anything it cannot attribute is reported, not excused.
    """
    name = None
    for match in FUNCTION.finditer(source, 0, offset):
        name = match.group("name")
    return name


def source_files(crate: str) -> list[pathlib.Path]:
    src = ROOT / "crates" / crate / "src"
    if not src.is_dir():
        raise SystemExit(f"{crate}: crates/{crate}/src does not exist")
    return sorted(
        path
        for path in src.rglob("*.rs")
        # `src/bin` SPECIFICALLY, not any path segment named "bin": a binary
        # crate root cannot reach the library's private `diag_env`, but a
        # library module that merely happens to live in a directory called
        # `bin` can, and excluding it would be a silent hole in a lint whose
        # whole value is that its denominator is the entire crate.
        if path.relative_to(src).parts[0] != "bin"
    )


def check_source(relative: str, source: str, in_diag_env_file: bool) -> list[str]:
    """Every impermissible env read in one file, as formatted failures."""
    failures = []
    for match in ENV_READ.finditer(source):
        line = source.count("\n", 0, match.start()) + 1
        if in_diag_env_file:
            function = enclosing_function(source, match.start())
            if function in PERMITTED_FUNCTIONS:
                continue
            failures.append(
                f"{relative}:{line}: env read in {function or '<module scope>'!s}, "
                f"but only {' and '.join(PERMITTED_FUNCTIONS)} may read the "
                "environment in this file"
            )
            continue
        failures.append(
            f"{relative}:{line}: direct process-environment read. In this crate the only "
            f"permitted call site is `{PERMITTED_FILE}`'s "
            f"{' / '.join(PERMITTED_FUNCTIONS)}. A `user`-class knob belongs on "
            "fn64-shell's `Knobs` and should be passed in as a value; a "
            "`diagnostic`-class one should read through `crate::diag_env`."
        )
    return failures


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="check the checker against synthetic sources and exit",
    )
    args = parser.parse_args(argv)
    if args.self_test:
        return self_test()

    failures: list[str] = []
    checked_files = 0
    permitted_sites = 0
    for crate in CRATES:
        seam = ROOT / "crates" / crate / "src" / PERMITTED_FILE
        # A crate that reads nothing needs no seam; one that does must have it
        # where this lint expects, or the "only permitted site" claim is empty.
        for path in source_files(crate):
            relative = str(path.relative_to(ROOT))
            source = path.read_text(encoding="utf-8")
            checked_files += 1
            in_seam = path == seam
            if in_seam:
                permitted_sites += len(ENV_READ.findall(source))
            failures.extend(check_source(relative, source, in_seam))

    if failures:
        print("\n".join(failures), file=sys.stderr)
        print(
            f"\n{len(failures)} impermissible environment read(s) across "
            f"{len(CRATES)} crates.",
            file=sys.stderr,
        )
        return 1
    print(
        f"hot-path env lint: {checked_files} library sources across {len(CRATES)} "
        f"crates read the environment only through {permitted_sites} permitted "
        f"call site(s) in {PERMITTED_FILE}"
    )
    return 0


def self_test() -> int:
    """Exercise the checker, including a planted direct read."""
    cases: list[tuple[str, str, bool, int]] = [
        # (label, source, in_diag_env_file, expected failure count)
        (
            "a plain module with no env read passes",
            "fn f() -> u32 { 7 }\n",
            False,
            0,
        ),
        (
            "a direct env::var in an ordinary module is rejected",
            "fn f() -> bool {\n    std::env::var(\"FN64_X\").is_ok()\n}\n",
            False,
            1,
        ),
        (
            "a direct env::var_os in an ordinary module is rejected",
            "fn f() -> bool {\n    std::env::var_os(\"FN64_X\").is_some()\n}\n",
            False,
            1,
        ),
        (
            "an unqualified env::var is rejected too",
            "use std::env;\nfn f() -> bool {\n    env::var(\"FN64_X\").is_ok()\n}\n",
            False,
            1,
        ),
        (
            "a read through the seam is not an env read",
            "fn f() -> bool {\n    crate::diag_env::diag_env(\"FN64_X\").is_some()\n}\n",
            False,
            0,
        ),
        (
            "diag_env.rs's own two functions may read",
            "pub(crate) fn diag_env(n: &'static str) -> Option<String> {\n"
            "    std::env::var(n).ok()\n}\n"
            "pub(crate) fn diag_env_present(n: &'static str) -> bool {\n"
            "    std::env::var_os(n).is_some()\n}\n",
            True,
            0,
        ),
        (
            "a read planted in some OTHER function of diag_env.rs is rejected",
            "pub(crate) fn diag_env(n: &'static str) -> Option<String> {\n"
            "    std::env::var(n).ok()\n}\n"
            "fn sneaky() -> bool {\n    std::env::var_os(\"FN64_X\").is_some()\n}\n",
            True,
            1,
        ),
        (
            "two reads in one file are both reported",
            "fn f() {\n    std::env::var(\"A\");\n    std::env::var_os(\"B\");\n}\n",
            False,
            2,
        ),
    ]

    failed = 0
    for label, source, in_seam, expected in cases:
        found = check_source("<self-test>", source, in_seam)
        if len(found) != expected:
            failed += 1
            print(
                f"SELF-TEST FAIL: {label}: expected {expected} failure(s), "
                f"got {len(found)}: {found}",
                file=sys.stderr,
            )
        else:
            print(f"SELF-TEST ok: {label}")

    if failed:
        print(f"{failed} self-test case(s) failed", file=sys.stderr)
        return 1
    print(f"hot-path env lint self-test: {len(cases)} cases passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
