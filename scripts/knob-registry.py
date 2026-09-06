#!/usr/bin/env python3
"""knob-registry -- the complete FN64_* environment-variable denominator.

Every `FN64_*` name the runtime reads is a live instruction: set it and
behavior changes (or should). Before this script the only catalog was
`docs/RT64-RUNTIME-CONTROLS.md`, which documents a fraction of the RT64-facing
subset and nothing else -- 288 distinct names are read across the workspace,
most of them uncatalogued. That gap is how a knob quietly goes dead: nothing
forces anyone to look at it again once it is wired.

This script is the mechanism, not the classification. It scans `crates/*/src`
(excluding any path segment containing "tests") for `FN64_[A-Z0-9_]+` tokens,
and for each distinct name reports: which crate it first appears in, the
`file:line` of that first appearance, how many times the name occurs in
non-test source, and a classification pulled from the hand-maintained
`docs/knobs.toml`.

It also records, per name, whether it is read at RUNTIME (`env::var`/
`var_os`, which can change behavior on every process launch) or at
BUILD TIME (`env!`/`option_env!`, baked into the binary by `rustc-env` and
immutable once compiled) -- or both. This distinction is why `build-time`
and `retired` exist as their own classes: a build-time-only name is not a
runtime knob at all (task 2.2's typed `Knobs` struct cannot hold it, because
there is nothing to hold -- the value is gone by the time any `Knobs`
constructor could run), and a `retired` name's only behavior is a loud
panic naming its replacement (AGENTS.md "loud traps, no silent shrugs";
deleting that read site would silently re-permit the exact footgun it
exists to close).

Three failure modes it refuses to pass silently on:
  - a name read in code with no entry in knobs.toml ("unknown") -- someone
    added a knob and never classified it.
  - a name in knobs.toml that no longer appears in code ("stale") -- someone
    deleted the read site and never removed the catalog entry, which is
    exactly the doc-drift class AGENTS.md's "mechanism over patch" targets.
  - a classification that contradicts its read kind: `build-time` with any
    runtime read, or `user`/`diagnostic`/`test-only`/`dead`/`retired` with
    ONLY build-time reads. Both are the same drift class as the other two --
    the classification says something the code does not support.

`--write` regenerates `docs/RUNTIME-KNOBS.md` from the current scan and exits
0. Without `--write` it recomputes the same table and exits nonzero if the
checked-in doc does not match byte-for-byte -- the same contract as
`lint-docs.py`'s other generated-doc checks (see `check_generated_validators`
there), and it is meant to be registered the same way.

Python 3 stdlib only -- no third-party deps, no `rg` dependency (unlike
lint-docs.py, which shells out to ripgrep; this walks the tree directly so it
has no external tool requirement on CI runners).
"""
from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
KNOBS_TOML = ROOT / "docs/knobs.toml"
GENERATED_DOC = ROOT / "docs/RUNTIME-KNOBS.md"
SCRIPT_REL = "scripts/knob-registry.py"

# A trailing underscore is never a complete name: it is always a truncated
# match on a PREFIX-FAMILY reference -- `name.starts_with("FN64_RELEASE_")`,
# a `"FN64_RELEASE_*"` namespace label, a `format!("FN64_DISCOVER_{label}_EVIDENCE")`
# interpolation, or a doc comment wrapped mid-identifier across two `///`
# lines (`write_barrier.rs`: "the count is counted. `FN64_MPROTECT_\n///
# BARRIER_SYSCALLS=1`"). Every concrete name these five prefixes actually
# gate (FN64_EXECUTABLE_IMAGE_MAIN, FN64_RELEASE_REPORT, FN64_MPROTECT_BARRIER_STATS,
# ...) is matched separately and correctly by this same regex; excluding the
# bare-prefix artifact loses no real knob.
NAME_RE = re.compile(r"\bFN64_[A-Z0-9_]*[A-Z0-9]\b")
VALID_CLASSES = {"user", "diagnostic", "test-only", "dead", "build-time", "retired"}
# Read-kind detection looks at what immediately precedes the name on the same
# line. `env!("FN64_X")` / `option_env!("FN64_X")` resolve at compile time
# and are baked into the binary by `rustc-env`; `env::var("FN64_X")` /
# `env::var_os("FN64_X")` (with or without the `std::` prefix) read the
# process environment at runtime, on every launch. A name can appear via
# both call shapes at different sites (checked across ALL occurrences, not
# just the first).
BUILD_TIME_CALL = re.compile(r"\b(?:std::)?option_env!\(\s*$|\benv!\(\s*$")
RUNTIME_CALL = re.compile(r"\b(?:std::)?env::var(?:_os)?\(\s*$")


class Occurrence:
    __slots__ = ("crate", "path", "line", "count", "runtime", "build_time")

    def __init__(self, crate: str, path: str, line: int):
        self.crate = crate
        self.path = path
        self.line = line
        self.count = 1
        self.runtime = False
        self.build_time = False

    def read_kind(self) -> str:
        if self.runtime and self.build_time:
            return "both"
        if self.build_time:
            return "build-time"
        if self.runtime:
            return "runtime"
        return "unknown"


def _is_test_path(rel: str) -> bool:
    """The task contract excludes any path CONTAINING "tests" -- not just a
    `tests/` directory segment. `private_input_admission_corpus_tests.rs` and
    plain `tests.rs` both need excluding, and a substring check catches both
    without needing a list of naming conventions."""
    return "tests" in rel


def scan_source() -> dict[str, Occurrence]:
    """First-occurrence file:line and total count per name, across
    crates/*/src, excluding any path with a `tests` path segment."""
    found: dict[str, Occurrence] = {}
    crates_dir = ROOT / "crates"
    for crate_dir in sorted(crates_dir.iterdir()):
        if not crate_dir.is_dir():
            continue
        src_dir = crate_dir / "src"
        if not src_dir.is_dir():
            continue
        crate = crate_dir.name
        for path in sorted(src_dir.rglob("*.rs")):
            rel = path.relative_to(ROOT).as_posix()
            if _is_test_path(path.relative_to(crate_dir).as_posix()):
                continue
            text = path.read_text(encoding="utf-8", errors="replace")
            for lineno, line in enumerate(text.splitlines(), 1):
                for match in NAME_RE.finditer(line):
                    name = match.group(0)
                    existing = found.get(name)
                    if existing is None:
                        existing = found[name] = Occurrence(crate, rel, lineno)
                    else:
                        existing.count += 1
                    # Only classify a read kind when the name sits inside a
                    # quoted string literal immediately preceded by one of
                    # the two call shapes -- `before` is everything on the
                    # line up to the opening quote, so `env!(\n    "FN64_X"`
                    # split across lines is intentionally NOT credited (rare
                    # in this codebase; a false "unknown" kind is safer than
                    # a false read-kind classification).
                    before = line[: match.start()]
                    quote_start = before.rfind('"')
                    if quote_start != -1:
                        prefix = before[:quote_start]
                        if BUILD_TIME_CALL.search(prefix):
                            existing.build_time = True
                        elif RUNTIME_CALL.search(prefix):
                            existing.runtime = True
    return found


def load_knobs_toml() -> dict[str, dict[str, str]]:
    """Minimal TOML reader for the exact shape knobs.toml uses:

        [FN64_NAME]
        class = "user"
        note = "..."

    Stdlib-only per the task contract. `tomllib` (3.11+) would be simpler,
    but this repo's floor is unspecified and the file's grammar is a single
    fixed shape, so a small hand parser avoids a version floor bump for one
    script.
    """
    if not KNOBS_TOML.exists():
        return {}
    entries: dict[str, dict[str, str]] = {}
    current: str | None = None
    section_re = re.compile(r"^\[([A-Za-z0-9_]+)\]\s*$")
    kv_re = re.compile(r'^([a-zA-Z_]+)\s*=\s*"((?:[^"\\]|\\.)*)"\s*$')
    for lineno, raw in enumerate(KNOBS_TOML.read_text(encoding="utf-8").splitlines(), 1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        section_match = section_re.match(line)
        if section_match:
            current = section_match.group(1)
            entries.setdefault(current, {})
            continue
        kv_match = kv_re.match(line)
        if kv_match and current is not None:
            key, value = kv_match.groups()
            entries[current][key] = value.replace('\\"', '"')
            continue
        raise ValueError(f"docs/knobs.toml:{lineno}: unparseable line {raw!r}")
    return entries


def render_doc(occurrences: dict[str, Occurrence], knobs: dict[str, dict[str, str]]) -> str:
    rows = sorted(occurrences.items(), key=lambda kv: (kv[1].crate, kv[0]))
    lines = [
        "<!-- GENERATED by scripts/knob-registry.py --write. Do not hand-edit. -->",
        "<!-- Classification source of truth: docs/knobs.toml -->",
        "# Runtime knob registry",
        "",
        f"{len(rows)} distinct `FN64_*` names read in non-test code under "
        "`crates/*/src`, one row per name. `class` and `note` come from "
        "`docs/knobs.toml`; regenerate this table with "
        "`python3 scripts/knob-registry.py --write` after editing that file.",
        "",
        "| Crate | Name | First site | Reads | Read kind | Class | Note |",
        "|---|---|---|---|---|---|---|",
    ]
    for name, occ in rows:
        entry = knobs.get(name, {})
        cls = entry.get("class", "")
        note = entry.get("note", "")
        lines.append(
            f"| {occ.crate} | `{name}` | `{occ.path}:{occ.line}` | {occ.count} "
            f"| {occ.read_kind()} | {cls} | {note} |"
        )
    lines.append("")
    return "\n".join(lines)


def main() -> int:
    write = "--write" in sys.argv

    occurrences = scan_source()
    knobs = load_knobs_toml()

    code_names = set(occurrences)
    toml_names = set(knobs)

    unknown = sorted(code_names - toml_names)
    stale = sorted(toml_names - code_names)

    errors: list[str] = []
    if unknown:
        errors.append(
            "unclassified FN64_* name(s) read in code but missing from docs/knobs.toml:\n"
            + "\n".join(f"  - {n} (first: {occurrences[n].path}:{occurrences[n].line})" for n in unknown)
        )
    for name in sorted(toml_names & code_names):
        cls = knobs[name].get("class")
        if cls not in VALID_CLASSES:
            errors.append(
                f"docs/knobs.toml [{name}]: class {cls!r} is not one of {sorted(VALID_CLASSES)}"
            )
            continue
        kind = occurrences[name].read_kind()
        if cls == "build-time" and kind in ("runtime", "both"):
            errors.append(
                f"docs/knobs.toml [{name}]: classified build-time but has a runtime "
                f"env::var/var_os read (kind={kind}) -- a build-time name must never "
                "be read at runtime, since task 2.2's Knobs struct cannot see it"
            )
        elif cls != "build-time" and kind == "build-time":
            errors.append(
                f"docs/knobs.toml [{name}]: classified {cls!r} but every read site is "
                "env!()/option_env!() (build-time only) -- reclassify as build-time"
            )
    if stale:
        errors.append(
            "stale docs/knobs.toml entr(y/ies): name no longer appears in crates/*/src:\n"
            + "\n".join(f"  - {n}" for n in stale)
        )

    if errors:
        for e in errors:
            print(e, file=sys.stderr)
        return 1

    doc_text = render_doc(occurrences, knobs)

    if write:
        GENERATED_DOC.write_text(doc_text, encoding="utf-8")
        print(f"knob-registry: wrote {GENERATED_DOC.relative_to(ROOT)} ({len(occurrences)} names)")
        return 0

    if not GENERATED_DOC.exists() or GENERATED_DOC.read_text(encoding="utf-8") != doc_text:
        print(
            f"knob-registry: {GENERATED_DOC.relative_to(ROOT)} is stale -- "
            "run `python3 scripts/knob-registry.py --write`",
            file=sys.stderr,
        )
        return 1

    print(f"knob-registry: clean ({len(occurrences)} names, doc up to date)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
