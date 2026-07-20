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


def check_generated_validators() -> None:
    validator = ROOT / "tools/check_unsupported_event_sites.py"
    result = subprocess.run(
        [sys.executable, str(validator)], cwd=ROOT, capture_output=True, text=True
    )
    if result.returncode != 0:
        fail("docs/unsupported-event-sites.json", result.stderr.strip() or result.stdout.strip())


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


def superseded(text: str) -> bool:
    """True if a doc marks ITSELF superseded with a top-of-file banner.

    A superseded design record documents the paths and env vars it PROPOSED
    or REJECTED, which by definition need not exist in code -- the drift
    rules ("a named file/var that isn't real is a silent no-op instruction")
    are about live, actionable docs, not history. The opt-out is a
    `> **SUPERSEDED` banner in the first few lines -- the doc's OWN status,
    not a doc that merely mentions the word elsewhere."""
    return any("**SUPERSEDED" in line for line in text.splitlines()[:5])


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
        text = doc.read_text()
        if superseded(text):
            continue
        for lineno, line in enumerate(text.splitlines(), 1):
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
        [
            "git",
            "grep",
            "-hoE",
            r"(FN64|OOT|RECOMP)_[A-Z0-9_]+",
            "--",
            "*.rs",
            "*.sh",
            "*.toml",
            "*.c",
            "*.cc",
            "*.cpp",
            "*.cxx",
            "*.h",
            "*.hpp",
        ],
        cwd=ROOT, capture_output=True, text=True,
    ).stdout
    live = set(src.split())
    for doc in docs():
        text = doc.read_text()
        if superseded(text):
            continue
        for lineno, line in enumerate(text.splitlines(), 1):
            for var in ENV.findall(line):
                # RECOMP_FUNC is a generated-C symbol prefix, not an env var.
                if var == "RECOMP_FUNC":
                    continue
                if var not in live:
                    rel = doc.relative_to(ROOT)
                    fail(f"{rel}:{lineno}", f"documents {var}, which appears nowhere in code")


# --- 3b. closed roadmap items stay short -------------------------------------
# A finished item's detail belongs in git, not in the doc every session reads.
# This check exists because prose did not work: the policy is stated IN
# ROADMAP.md, and the session that wrote it then filed two 15-line completion
# records within the hour. An invariant enforced by review is a bug with a
# delay timer (AGENTS.md), so it is enforced here instead.
CLOSED_ITEM_MAX_LINES = 4


def _scan_closed_items(lines: list[str]) -> None:
    start, label = None, ""
    for i, line in enumerate(lines):
        # An item ends at the next item, or at the next top-level block.
        if start is not None and (line.startswith("- [") or (line and not line[0].isspace())):
            if i - start > CLOSED_ITEM_MAX_LINES:
                fail(
                    f"docs/ROADMAP.md:{start + 1}",
                    f"closed item {label!r} is {i - start} lines (max {CLOSED_ITEM_MAX_LINES}) -- "
                    f"summarize it; the detail is in git",
                )
            start = None
        if line.startswith("- [x]"):
            start, label = i, line[5:47].strip(" *")


def check_closed_roadmap_items() -> None:
    roadmap = ROOT / "docs/ROADMAP.md"
    if roadmap.exists():
        _scan_closed_items(roadmap.read_text().splitlines())


# --- 3c. a doc asserting a hash must have a test that checks it ---------------
# A SHA in a doc claims a REPRODUCIBLE fact ("these two lanes render
# byte-identically at swap 499"). Verified once by hand and never again, it is
# decorative: it cannot fail, so it cannot warn. All 5 hashes in docs/ were
# unbacked when this check was written (ROADMAP V1) -- including the framebuffer
# SHA that is fn64's central c/rs lane-parity claim.
#
# ponytail: a hash is either load-bearing (then a test owns it) or prose (then
# do not write it as evidence). This check forces that choice.
HASH = re.compile(r"\b[0-9a-f]{40,64}\b")


def check_doc_hashes_are_tested() -> None:
    src = subprocess.run(
        ["git", "grep", "-hoE", r"[0-9a-f]{40,64}", "--", "*.rs", "*.sh", "*.py"],
        cwd=ROOT, capture_output=True, text=True,
    ).stdout
    tested = set(src.split())
    for doc in docs():
        for lineno, line in enumerate(doc.read_text().splitlines(), 1):
            low = line.lower()
            # A COMMIT PIN is provenance ("checked against CEN64 at e0641c8"),
            # not a reproducible measurement -- it is exactly the clean-room
            # citation AGENTS.md requires, and no test should own it. Only
            # CONTENT hashes (a framebuffer SHA, a golden digest) assert a fact
            # a test can re-check. Distinguish by context, not by the hash.
            if any(k in low for k in ("commit", "github.com", "pinned", "tree/", "blob/")):
                continue
            # An explicitly-unverified claim is honest prose, not a false gate.
            if "not tested" in low or "no test" in low:
                continue
            for h in HASH.findall(line):
                if h in tested:
                    continue
                rel = doc.relative_to(ROOT)
                fail(
                    f"{rel}:{lineno}",
                    f"asserts content hash {h[:12]}… that no test checks -- either gate it "
                    f"or stop citing it as evidence",
                )


# --- 4. COMPLETENESS's generated ABI inventory must match live code ----------
# It once grepped lib.rs alone; the crate had been split into modules, so it
# matched ZERO of 73 shims and "regenerating" re-asserted a falsehood. The
# dedicated checker now owns both the clean-room 116-name manifest and the
# generated doc block; run it here so every ordinary doc-lint invocation is a
# mechanical surface-drift gate.
def check_completeness_recipe() -> None:
    result = subprocess.run(
        [sys.executable, str(ROOT / "scripts/check-nmr-surface.py"), "--check-doc"],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip() or "checker failed silently"
        fail("COMPLETENESS.md", detail)
    elif VERBOSE:
        print("  NMR surface manifest, live ABI, and completeness doc agree")


# --- 4b. RT64's advertised feature denominator must remain complete ---------
# The ordinary doc gate checks schema, rejection guards, evidence shape, and
# generated-doc drift without requiring an external checkout. Release owners
# additionally pass --rt64-dir directly to the inventory tool to prove the
# pinned source identity and line anchors.
def check_rt64_feature_inventory() -> None:
    result = subprocess.run(
        [sys.executable, str(ROOT / "tools/check_rt64_feature_inventory.py")],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip() or "checker failed silently"
        fail("RT64-PUBLIC-FEATURE-INVENTORY.md", detail)
    elif VERBOSE:
        print("  RT64 public feature manifest and generated inventory agree")


# --- 4bb. cross-platform RT64 case/blocker matrix must not shrink ----------
def check_rt64_platform_certification() -> None:
    result = subprocess.run(
        [sys.executable, str(ROOT / "tools/rt64_platform_certification.py"), "--check"],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip() or "checker failed silently"
        fail("RT64-PLATFORM-CERTIFICATION.md", detail)
    elif VERBOSE:
        print("  RT64 platform/API case and blocker denominators agree")


# --- 4bc. private inputs stay external while the admission contract works --
def check_private_input_admission() -> None:
    result = subprocess.run(
        [sys.executable, str(ROOT / "tools/private_input_admission.py"), "--check"],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip() or "checker failed silently"
        fail("PRIVATE-INPUT-ADMISSION.md", detail)
    elif VERBOSE:
        print("  private-input admission rejects identity/content leakage")


# --- 4c. base-renderer accuracy is a generated, non-shrinking denominator --
def check_base_renderer_matrix() -> None:
    result = subprocess.run(
        [sys.executable, str(ROOT / "tools/check_base_renderer_matrix.py")],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip() or "checker failed silently"
        fail("BASE-RENDERER-BEHAVIOR-MATRIX.md", detail)
    elif VERBOSE:
        print("  base-renderer behavior matrix and generated report agree")


# --- 5. no doc may cite a scripts/ entry point that isn't executable ---------
def check_scripts() -> None:
    for doc in docs():
        for raw in REF.findall(doc.read_text()):
            ref = raw.rstrip(TRAIL)
            if ref.startswith("scripts/") and ref.endswith(".sh"):
                p = ROOT / ref
                if p.exists() and not p.stat().st_mode & 0o111:
                    fail(str(doc.relative_to(ROOT)), f"cites {ref}, which is not executable")


def _closed_item_check_fires() -> bool:
    """Feed the closed-item check a bloated item and assert it complains.
    Reconstructs the exact mistake this check exists to catch: a [x] item that
    carries its verification detail instead of a summary."""
    global errors
    saved, errors = errors, []
    try:
        bloated = [
            "- [x] **H1 vendor `recomp.h`** (2026-07-17) — in-tree.",
            "  Verified: the C lane builds with RECOMP_H_DIR unset;",
            "  override still honored; 621/621 workspace tests pass;",
            "  vendored copy byte-identical to upstream a8e2200.",
            "  Two things the inventory got wrong, found by doing it:",
            "  only recomp.h was needed, and oot.toml must not move.",
            "- [ ] **H3 next item**",
        ]
        _scan_closed_items(bloated)
        return len(errors) == 1
    finally:
        errors = saved


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
        ("closed-item cap fires", "a bloated [x] item must fail",
         _closed_item_check_fires),
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
               check_closed_roadmap_items, check_doc_hashes_are_tested,
               check_completeness_recipe, check_rt64_feature_inventory,
               check_rt64_platform_certification,
               check_private_input_admission,
               check_base_renderer_matrix,
               check_generated_validators,
               check_scripts):
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
