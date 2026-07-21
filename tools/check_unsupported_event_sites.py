#!/usr/bin/env python3
"""Validate the typed unsupported-site registry and its source instrumentation."""

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
REGISTRY = ROOT / "docs/unsupported-event-sites.json"
SOURCE_ROOTS = [
    ROOT / "crates/fn64-runtime/src",
    ROOT / "crates/fn64-abi/src",
    ROOT / "crates/fn64-audio/src",
    ROOT / "crates/fn64-recomp-rs/src",
    ROOT / "crates/fn64-render/src",
    ROOT / "crates/fn64-render-reference/src",
    ROOT / "crates/fn64-render-rt64/src",
]
CALL = re.compile(
    r"record_unsupported_event\(\s*[^,]+,\s*\"([^\"]+)\"", re.DOTALL
)
HELPER_CALLS = (
    re.compile(r"render_unsupported_error\(\s*[^,]+,\s*\"([^\"]+)\"", re.DOTALL),
    re.compile(r"render_unsupported_panic\(\s*\"([^\"]+)\"", re.DOTALL),
    re.compile(r"\bunsupported\(\s*\"([^\"]+)\"", re.DOTALL),
)
RECORDED_OUTCOME_FORMS = (
    "record_unsupported_event",
    "trap_unsupported",
    "trap_unknown",
    "trap_unknown_vu",
    "trap_delay_slot_control",
    "unsupported_error",
    "unsupported_panic",
    "unsupported(",
)


def brace_delta(line: str) -> int:
    """Count structural braces while ignoring quoted strings and line comments."""
    delta = 0
    quoted = None
    escaped = False
    index = 0
    while index < len(line):
        char = line[index]
        if quoted is not None:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == quoted:
                quoted = None
            index += 1
            continue
        if line[index : index + 2] == "//":
            break
        if char in {'"', "'"}:
            quoted = char
        elif char == "{":
            delta += 1
        elif char == "}":
            delta -= 1
        index += 1
    return delta


def production_lines(text: str) -> list[tuple[int, str]]:
    """Drop complete items gated by cfg(test), including inline test modules."""
    output: list[tuple[int, str]] = []
    pending_test_item = False
    skipping = False
    saw_body = False
    depth = 0
    for lineno, line in enumerate(text.splitlines(), 1):
        stripped = line.strip()
        if not skipping and stripped in {"#[cfg(test)]", "#[test]"}:
            pending_test_item = True
            continue
        if pending_test_item and not skipping:
            if stripped.startswith("#["):
                continue
            skipping = True
        if skipping:
            change = brace_delta(line)
            if change > 0:
                saw_body = True
            depth += change
            if (saw_body and depth <= 0) or (not saw_body and stripped.endswith(";")):
                pending_test_item = False
                skipping = False
                saw_body = False
                depth = 0
            continue
        output.append((lineno, line))
    return output


def fail(message: str) -> None:
    print(f"unsupported-event-sites: {message}", file=sys.stderr)
    raise SystemExit(1)


def main() -> None:
    document = json.loads(REGISTRY.read_text())
    if document.get("schema") != "fn64.unsupported-event-sites.v2":
        fail("wrong or missing schema")
    families = document.get("families")
    if not isinstance(families, list) or not families:
        fail("families must be a non-empty list")
    sites = document.get("sites")
    if not isinstance(sites, list) or not sites:
        fail("sites must be a non-empty list")

    registered: dict[str, dict[str, str]] = {}
    for index, site in enumerate(sites):
        required = {"operation", "subsystem", "disposition", "path", "trigger"}
        if set(site) != required:
            fail(f"site {index} fields differ from {sorted(required)}")
        operation = site["operation"]
        if operation in registered:
            fail(f"duplicate operation {operation!r}")
        if site["subsystem"] not in {
            "runtime",
            "abi",
            "audio",
            "recompiler",
            "render",
        }:
            fail(f"{operation}: invalid subsystem {site['subsystem']!r}")
        if site["disposition"] not in {"loud_trap", "returned_error", "needs_lle"}:
            fail(f"{operation}: invalid disposition {site['disposition']!r}")
        path = ROOT / site["path"]
        if not path.is_file():
            fail(f"{operation}: missing source {site['path']}")
        text = "\n".join(line for _, line in production_lines(path.read_text()))
        if text.count(site["trigger"]) != 1:
            fail(f"{operation}: trigger must occur exactly once in {site['path']}")
        if text.count(f'"{operation}"') != 1:
            fail(f"{operation}: operation literal must occur exactly once in {site['path']}")
        registered[operation] = site

    family_operations: set[str] = set()
    for index, family in enumerate(families):
        required = {
            "operation_prefix",
            "subsystem",
            "dispositions",
            "recorder_path",
            "recorder_trigger",
            "source_paths",
        }
        if set(family) != required:
            fail(f"family {index} fields differ from {sorted(required)}")
        prefix = family["operation_prefix"]
        if not isinstance(prefix, str) or not prefix.endswith("."):
            fail(f"family {index}: operation_prefix must end in '.'")
        if family["subsystem"] != "render":
            fail(f"family {index}: unsupported subsystem {family['subsystem']!r}")
        if sorted(family["dispositions"]) != ["loud_trap", "returned_error"]:
            fail(f"family {index}: dispositions must cover loud_trap and returned_error")
        recorder = ROOT / family["recorder_path"]
        if not recorder.is_file():
            fail(f"family {index}: missing recorder {family['recorder_path']}")
        recorder_text = "\n".join(
            line for _, line in production_lines(recorder.read_text())
        )
        if recorder_text.count(family["recorder_trigger"]) != 1:
            fail(f"family {index}: recorder trigger must occur exactly once")
        source_paths = family["source_paths"]
        if not isinstance(source_paths, list) or not source_paths:
            fail(f"family {index}: source_paths must be non-empty")
        found = set()
        for relative in source_paths:
            path = ROOT / relative
            if not path.is_file():
                fail(f"family {index}: missing source {relative}")
            text = "\n".join(line for _, line in production_lines(path.read_text()))
            for pattern_index, pattern in enumerate(HELPER_CALLS):
                operations = set(pattern.findall(text))
                if pattern_index == 2:
                    operations = {
                        operation for operation in operations if operation.startswith(prefix)
                    }
                found.update(operations)
        if not found:
            fail(f"family {index}: no literal helper operations found")
        wrong_prefix = sorted(operation for operation in found if not operation.startswith(prefix))
        if wrong_prefix:
            fail(f"family {index}: operations outside prefix: {wrong_prefix}")
        overlap = sorted(found & set(registered))
        if overlap:
            fail(f"family {index}: helper operations duplicate exact sites: {overlap}")
        family_operations.update(found)

    observed: dict[str, str] = {}
    for root in SOURCE_ROOTS:
        for path in root.rglob("*.rs"):
            if path.name == "unsupported.rs":
                continue
            text = "\n".join(line for _, line in production_lines(path.read_text()))
            for operation in CALL.findall(text):
                relative = str(path.relative_to(ROOT))
                if operation in observed:
                    fail(f"{operation}: recorded in both {observed[operation]} and {relative}")
                observed[operation] = relative

    missing = sorted(set(registered) - set(observed))
    extra = sorted(set(observed) - set(registered))
    if missing or extra:
        fail(f"registry/source mismatch: missing={missing}, unregistered={extra}")
    for operation, relative in observed.items():
        if registered[operation]["path"] != relative:
            fail(
                f"{operation}: registry path {registered[operation]['path']} != source {relative}"
            )

    # Sweep the loud shapes most likely to bypass the typed source. Existing
    # executable unsupported messages must be a registered trigger; comments,
    # enum declarations, and test assertions are intentionally excluded.
    for root in SOURCE_ROOTS:
        for path in root.rglob("*.rs"):
            if path.name == "unsupported.rs" or "/tests/" in str(path):
                continue
            lines = production_lines(path.read_text())
            for position, (lineno, line) in enumerate(lines):
                stripped = line.strip()
                if stripped.startswith(("//", "///", "//!")):
                    continue
                if "unsupported" in line.lower() and any(
                    token in line for token in ("panic!", "format!(", 'reason: "')
                ):
                    window = "\n".join(
                        candidate
                        for _, candidate in lines[max(0, position - 6) : position + 7]
                    )
                    if not any(form in window for form in RECORDED_OUTCOME_FORMS):
                        fail(
                            f"{path.relative_to(ROOT)}:{lineno}: "
                            "unsupported outcome bypasses a typed recorder"
                        )
    print(
        "unsupported-event-sites: ok "
        f"({len(sites)} exact sites, {len(family_operations)} family operations)"
    )


if __name__ == "__main__":
    main()
