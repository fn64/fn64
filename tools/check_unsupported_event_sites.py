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
DYNAMIC_FORMAT_CALL = re.compile(
    r"record_unsupported_event\(\s*[^,]+,\s*format!\(\s*\"([^\"]+)\"",
    re.DOTALL,
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
OUTCOME_MACROS = re.compile(r"\b(panic|unimplemented|todo|format)!\s*\(")
UNSUPPORTED_OUTCOME = re.compile(
    r"\b(?:unsupported|unimplemented)\b|\bnot\s+(?:implemented|supported)\b",
    re.IGNORECASE,
)
VALID_SUBSYSTEMS = {"runtime", "abi", "audio", "recompiler", "render"}
VALID_DISPOSITIONS = {"loud_trap", "returned_error", "needs_lle"}


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


def macro_invocations(lines: list[tuple[int, str]]) -> list[tuple[int, int, str, str]]:
    """Return balanced outcome-macro calls as line-index spans and source text."""
    text = "\n".join(line for _, line in lines)
    invocations = []
    for match in OUTCOME_MACROS.finditer(text):
        depth = 1
        index = match.end()
        quoted = None
        escaped = False
        block_comment_depth = 0
        while index < len(text) and depth > 0:
            char = text[index]
            pair = text[index : index + 2]
            if block_comment_depth:
                if pair == "/*":
                    block_comment_depth += 1
                    index += 2
                    continue
                if pair == "*/":
                    block_comment_depth -= 1
                    index += 2
                    continue
                index += 1
                continue
            if quoted is not None:
                if escaped:
                    escaped = False
                elif char == "\\":
                    escaped = True
                elif char == quoted:
                    quoted = None
                index += 1
                continue
            if pair == "//":
                newline = text.find("\n", index + 2)
                index = len(text) if newline < 0 else newline + 1
                continue
            if pair == "/*":
                block_comment_depth = 1
                index += 2
                continue
            if char in {'"', "'"}:
                quoted = char
            elif char == "(":
                depth += 1
            elif char == ")":
                depth -= 1
            index += 1
        if depth != 0:
            continue
        start_line = text.count("\n", 0, match.start())
        end_line = text.count("\n", 0, index)
        if lines[start_line][1].lstrip().startswith(("//", "///", "//!")):
            continue
        invocations.append((start_line, end_line, match.group(1), text[match.start() : index]))
    return invocations


def unrecorded_outcomes(lines: list[tuple[int, str]]) -> list[tuple[int, str]]:
    """Find unsupported terminal/error construction that lacks a nearby recorder."""
    failures = []
    for start, end, macro, invocation in macro_invocations(lines):
        if macro not in {"unimplemented", "todo"} and not UNSUPPORTED_OUTCOME.search(invocation):
            continue
        window = "\n".join(
            candidate
            for _, candidate in lines[max(0, start - 12) : min(len(lines), end + 13)]
        )
        if not any(form in window for form in RECORDED_OUTCOME_FORMS):
            failures.append((lines[start][0], macro))
    return failures


def dynamic_operation_prefix(format_string: str) -> str | None:
    """Return the stable prefix before a Rust format field, if one exists."""
    field = format_string.find("{")
    if field <= 0:
        return None
    return format_string[:field]


def registered_dynamic_operation_prefix(
    format_string: str, registered_prefixes: set[str]
) -> str | None:
    prefix = dynamic_operation_prefix(format_string)
    return prefix if prefix in registered_prefixes else None


def selftest(announce: bool = True) -> None:
    bad = production_lines(
        '''fn reject() {
    panic!(
        "operation is not implemented"
    );
}'''
    )
    assert unrecorded_outcomes(bad) == [(2, "panic")]

    bad_error = production_lines(
        '''fn reject() -> Result<(), String> {
    Err(format!(
        "unsupported command {opcode}"
    ))
}'''
    )
    assert unrecorded_outcomes(bad_error) == [(2, "format")]

    recorded = production_lines(
        '''fn reject() {
    record_unsupported_event(
        UnsupportedSubsystem::Abi,
        "abi.test.rejected",
        "context",
        None,
        UnsupportedDisposition::LoudTrap,
    );
    panic!(
        "operation is unsupported"
    );
}'''
    )
    assert unrecorded_outcomes(recorded) == []

    tests_only = production_lines(
        '''#[cfg(test)]
fn fixture() {
    panic!("unsupported fixture");
}
fn production() {}'''
    )
    assert unrecorded_outcomes(tests_only) == []
    assert dynamic_operation_prefix("abi.si.voice-command-{command:02x}") == (
        "abi.si.voice-command-"
    )
    assert dynamic_operation_prefix("render.static") is None
    dynamic_prefixes = {"abi.si.voice-command-", "abi.si.pif-command-"}
    assert registered_dynamic_operation_prefix(
        "abi.si.voice-command-{command:02x}", dynamic_prefixes
    ) == "abi.si.voice-command-"
    assert (
        registered_dynamic_operation_prefix(
            "abi.si.unregistered-command-{command:02x}", dynamic_prefixes
        )
        is None
    )
    if announce:
        print("unsupported-event-sites selftest: ok")


def fail(message: str) -> None:
    print(f"unsupported-event-sites: {message}", file=sys.stderr)
    raise SystemExit(1)


def main() -> None:
    selftest(announce=False)
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
        if site["subsystem"] not in VALID_SUBSYSTEMS:
            fail(f"{operation}: invalid subsystem {site['subsystem']!r}")
        if site["disposition"] not in VALID_DISPOSITIONS:
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
    family_prefixes: set[str] = set()
    dynamic_family_prefixes: set[str] = set()
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
        subsystem = family["subsystem"]
        if subsystem not in VALID_SUBSYSTEMS:
            fail(f"family {index}: invalid subsystem {subsystem!r}")
        if not isinstance(prefix, str) or not prefix.startswith(f"{subsystem}."):
            fail(f"family {index}: operation_prefix must begin with {subsystem!r} plus '.'")
        if prefix in family_prefixes:
            fail(f"family {index}: duplicate operation_prefix {prefix!r}")
        dispositions = family["dispositions"]
        if (
            not isinstance(dispositions, list)
            or not dispositions
            or len(dispositions) != len(set(dispositions))
            or not set(dispositions) <= VALID_DISPOSITIONS
        ):
            fail(f"family {index}: dispositions must be a unique non-empty supported set")
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
        found_dynamic = set()
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
            for format_string in DYNAMIC_FORMAT_CALL.findall(text):
                dynamic_prefix = dynamic_operation_prefix(format_string)
                if dynamic_prefix == prefix:
                    found_dynamic.add(dynamic_prefix)
        if not found and not found_dynamic:
            fail(f"family {index}: no literal helper operations or dynamic recorder found")
        wrong_prefix = sorted(operation for operation in found if not operation.startswith(prefix))
        if wrong_prefix:
            fail(f"family {index}: operations outside prefix: {wrong_prefix}")
        overlap = sorted(found & set(registered))
        if overlap:
            fail(f"family {index}: helper operations duplicate exact sites: {overlap}")
        family_operations.update(found)
        family_prefixes.add(prefix)
        dynamic_family_prefixes.update(found_dynamic)

    observed: dict[str, str] = {}
    observed_dynamic: dict[str, str] = {}
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
            for format_string in DYNAMIC_FORMAT_CALL.findall(text):
                prefix = dynamic_operation_prefix(format_string)
                if prefix is None:
                    fail(
                        f"{relative}: dynamic unsupported operation format {format_string!r} "
                        "has no stable literal prefix"
                    )
                if (
                    registered_dynamic_operation_prefix(
                        format_string, dynamic_family_prefixes
                    )
                    is None
                ):
                    fail(
                        f"{relative}: unregistered dynamic unsupported operation family "
                        f"{format_string!r}"
                    )
                if prefix in observed_dynamic:
                    fail(
                        f"dynamic family {prefix!r}: recorded in both "
                        f"{observed_dynamic[prefix]} and {relative}"
                    )
                observed_dynamic[prefix] = relative

    missing = sorted(set(registered) - set(observed))
    extra = sorted(set(observed) - set(registered))
    if missing or extra:
        fail(f"registry/source mismatch: missing={missing}, unregistered={extra}")
    for operation, relative in observed.items():
        if registered[operation]["path"] != relative:
            fail(
                f"{operation}: registry path {registered[operation]['path']} != source {relative}"
            )

    missing_dynamic = sorted(dynamic_family_prefixes - set(observed_dynamic))
    if missing_dynamic:
        fail(f"registered dynamic families have no recorder: {missing_dynamic}")

    # Sweep the loud shapes most likely to bypass the typed source. Balanced
    # macro extraction keeps multiline panic/error construction visible while
    # cfg(test) items, comments, enum declarations, and assertions stay out.
    for root in SOURCE_ROOTS:
        for path in root.rglob("*.rs"):
            relative = path.relative_to(ROOT)
            if (
                path.name == "unsupported.rs"
                or "tests" in relative.parts
                or ("src" in relative.parts and "bin" in relative.parts)
            ):
                continue
            for lineno, macro in unrecorded_outcomes(production_lines(path.read_text())):
                fail(
                    f"{relative}:{lineno}: unsupported {macro}! outcome "
                    "bypasses a typed recorder"
                )
    print(
        "unsupported-event-sites: ok "
        f"({len(sites)} exact sites, {len(family_operations)} literal family operations, "
        f"{len(family_prefixes)} operation families, "
        f"{len(dynamic_family_prefixes)} dynamic)"
    )


if __name__ == "__main__":
    if sys.argv[1:] == ["--selftest"]:
        selftest()
    elif sys.argv[1:]:
        fail("usage: check_unsupported_event_sites.py [--selftest]")
    else:
        main()
