#!/usr/bin/env python3
"""lint-docs -- catch the doc-drift class AGENTS.md's "mechanism over patch" wants.

Docs here are load-bearing: agents follow them to spec. So a doc that points at
a file that doesn't exist, or names a crate that isn't real, is a live bug that
sends the next session down a wrong path. On 2026-07-17 all five checks below
were failing at once -- AGENTS.md read-order item 3 pointed at a
`docs/ABI-SURFACE.md` that never existed, DECOUPLING told agents to build a
crate the project deliberately doesn't have, COMPLETENESS asserted an ABI
family was ABSENT while the code implemented it, and README described 5 of 11
crates. Each was found by a human asking the right question. That doesn't scale;
this does.

Usage: scripts/lint-docs.py [--verbose]   (exit 0 clean, 1 on any error)

ponytail: five greps and a file-exists check, no schema, no deps. If this ever
needs a real graph model, the answer is `keel` (github.com/jer/keel), not a
bigger script.
"""
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
VERBOSE = "--verbose" in sys.argv
errors: list[str] = []
checked = 0


def fail(where: str, msg: str) -> None:
    errors.append(f"{where}: {msg}")


def ignored(ref: str) -> bool:
    """True if .gitignore covers this path -- ask git, don't reimplement it."""
    return subprocess.run(
        ["git", "check-ignore", "-q", ref], cwd=ROOT, capture_output=True
    ).returncode == 0


def docs() -> list[Path]:
    """Every tracked markdown file (untracked scratch and vendored trees are noise)."""
    out = subprocess.run(
        ["git", "ls-files", "*.md"], cwd=ROOT, capture_output=True, text=True, check=True
    )
    return [ROOT / p for p in out.stdout.split()]


# --- 1. every repo-relative path a doc names must exist -----------------------
# Both shapes occur: `docs/X.md` in backticks (the common one) and bare
# docs/X.md. The leading class must therefore ALLOW a backtick and only reject
# a preceding path char -- an earlier version excluded backticks and so matched
# 4 refs instead of 24, passing green while blind. Regression-tested below.
REF = re.compile(r"(?:^|[^\w/.-])((?:docs|crates|scripts|examples)/[\w./-]+)")
# Trailing punctuation bleeds into the match; strip it before testing.
TRAIL = ".,;:)]}'\""


def check_refs() -> None:
    global checked
    for doc in docs():
        for lineno, line in enumerate(doc.read_text().splitlines(), 1):
            for raw in REF.findall(line):
                ref = raw.rstrip(TRAIL)
                # Illustrative globs/wildcards aren't claims about one file.
                if "*" in ref or ref.endswith("/"):
                    continue
                # Build artifacts (target/, recompiled/) exist only after a
                # build and are gitignored; a doc naming one is telling you to
                # delete or inspect it, not claiming it's checked in.
                if ignored(ref):
                    continue
                checked += 1
                if not (ROOT / ref).exists():
                    rel = doc.relative_to(ROOT)
                    fail(f"{rel}:{lineno}", f"references {ref} which does not exist")


# --- 2. every workspace member appears in README's crate table ---------------
def check_readme_crates() -> None:
    manifest = (ROOT / "Cargo.toml").read_text()
    members = re.findall(r"fn64-[\w-]+", manifest.split("members = [")[1].split("]")[0])
    readme = (ROOT / "README.md").read_text()
    for crate in members:
        if crate not in readme:
            fail("README.md", f"workspace member {crate} is not mentioned")


# --- 3. every documented env var exists in code ------------------------------
# A doc naming a var the code never reads is an instruction that silently no-ops.
ENV = re.compile(r"\b((?:FN64|OOT|RECOMP)_[A-Z0-9_]+)\b")


def check_env_vars() -> None:
    src = subprocess.run(
        ["git", "grep", "-hoE", r"(FN64|OOT|RECOMP)_[A-Z0-9_]+", "--", "*.rs", "*.sh", "*.toml", "*.c"],
        cwd=ROOT, capture_output=True, text=True,
    ).stdout
    live = set(src.split())
    for doc in docs():
        for lineno, line in enumerate(doc.read_text().splitlines(), 1):
            for var in ENV.findall(line):
                # RECOMP_FUNC is a generated-C symbol prefix, not an env var.
                if var == "RECOMP_FUNC":
                    continue
                if var not in live:
                    rel = doc.relative_to(ROOT)
                    fail(f"{rel}:{lineno}", f"documents {var}, which appears nowhere in code")


# --- 4. COMPLETENESS's own regen recipe must not be blind --------------------
# It once grepped lib.rs alone; the crate had been split into modules, so it
# matched ZERO of 73 shims and "regenerating" re-asserted a falsehood. A doc
# whose maintenance tool is broken rots faster than one with no tool.
def check_completeness_recipe() -> None:
    shims = subprocess.run(
        ["git", "grep", "-hc", "_recomp(", "--", "crates/fn64-abi/src/"],
        cwd=ROOT, capture_output=True, text=True,
    ).stdout
    total = sum(int(n) for n in shims.split() if n.isdigit())
    if total == 0:
        fail("COMPLETENESS.md", "regen recipe finds 0 shims in crates/fn64-abi/src/ -- recipe is blind")
    elif VERBOSE:
        print(f"  completeness recipe sees {total} shims")


# --- 5. no doc may cite a scripts/ entry point that isn't executable ---------
def check_scripts() -> None:
    for doc in docs():
        for raw in REF.findall(doc.read_text()):
            ref = raw.rstrip(TRAIL)
            if ref.startswith("scripts/") and ref.endswith(".sh"):
                p = ROOT / ref
                if p.exists() and not p.stat().st_mode & 0o111:
                    fail(str(doc.relative_to(ROOT)), f"cites {ref}, which is not executable")


def selftest() -> int:
    """Prove the checks can FAIL. A linter that passes because it isn't looking
    is worse than none -- the first cut of REF excluded backticks, matched 4
    refs instead of 53, and reported clean while blind to every real bug."""
    global errors
    cases = [
        ("dangling ref", "AGENTS.md:1: references docs/NOPE.md",
         lambda: REF.findall("see `docs/NOPE.md` for more") == ["docs/NOPE.md"]),
        ("backticked ref matches", "the blindness regression",
         lambda: REF.findall("`crates/fn64-abi/src/lib.rs`") == ["crates/fn64-abi/src/lib.rs"]),
        ("bare ref matches", "unbackticked form",
         lambda: REF.findall("run scripts/native-emit.sh now") == ["scripts/native-emit.sh"]),
        ("glob skipped", "illustrative wildcards aren't claims",
         lambda: all("*" in r for r in REF.findall("`crates/*/tests/`"))),
        ("env var regex", "catches a fake var",
         lambda: ENV.findall("set `FN64_TOTALLY_FAKE=1`") == ["FN64_TOTALLY_FAKE"]),
    ]
    bad = [name for name, why, fn in cases if not fn()]
    for name in bad:
        print(f"  SELFTEST FAIL: {name}")
    if bad:
        return 1
    print(f"lint-docs selftest: {len(cases)} checks can fail correctly")
    return 0


def main() -> int:
    if "--selftest" in sys.argv:
        return selftest()
    for fn in (check_refs, check_readme_crates, check_env_vars,
               check_completeness_recipe, check_scripts):
        fn()
    if errors:
        print(f"lint-docs: {len(errors)} error(s)\n")
        for e in errors:
            print(f"  {e}")
        return 1
    print(f"lint-docs: clean ({checked} refs across {len(docs())} docs)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
