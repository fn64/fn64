#!/usr/bin/env python3
"""Keep fn64-discover's test-harness targets explicit and minimal."""

import re
import sys
import tomllib
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
CRATE = ROOT / "crates/fn64-discover"
MANIFEST = CRATE / "Cargo.toml"
TEST_MARKER = re.compile(r"(?m)^\s*#\s*\[\s*(?:cfg\s*\(\s*test\s*\)|test)\s*\]")
EXPECTED_TEST_BINS = {
    "compare_computed_flows",
    "gate_owners_overlays",
    "gate_recompiler_lint",
    "gate_wm2000_recompile",
    "ingest_tool_claims",
    "produce_snapshot_workspace",
    "rom_identity",
    "run_wm_writer_audit",
    "validate_executable_image_group",
}


def main() -> int:
    manifest = tomllib.loads(MANIFEST.read_text())
    errors: list[str] = []
    if manifest["package"].get("autobins") is not False:
        errors.append("package.autobins must be false")

    source_paths = {
        path.relative_to(CRATE).as_posix(): path
        for path in sorted((CRATE / "src/bin").glob("*.rs"))
    }
    declared: dict[str, dict[str, object]] = {}
    names: set[str] = set()
    for target in manifest.get("bin", []):
        name = target.get("name")
        path = target.get("path")
        if not isinstance(name, str) or not isinstance(path, str):
            errors.append("every [[bin]] needs string name and path")
            continue
        if name in names:
            errors.append(f"duplicate bin name: {name}")
        if path in declared:
            errors.append(f"duplicate bin path: {path}")
        names.add(name)
        declared[path] = target

    for path in sorted(source_paths.keys() - declared.keys()):
        errors.append(f"undeclared bin source: {path}")
    for path in sorted(declared.keys() - source_paths.keys()):
        errors.append(f"bin path has no source: {path}")

    marked_names: set[str] = set()
    for path in sorted(source_paths.keys() & declared.keys()):
        target = declared[path]
        name = str(target["name"])
        has_tests = TEST_MARKER.search(source_paths[path].read_text()) is not None
        if has_tests:
            marked_names.add(name)
        test_enabled = target.get("test")
        if not isinstance(test_enabled, bool):
            errors.append(f"{name}: test must be explicitly true or false")
        elif test_enabled != has_tests:
            state = "enabled" if test_enabled else "disabled"
            marker = "has" if has_tests else "has no"
            errors.append(f"{name}: test is {state} but source {marker} unit-test marker")

    if marked_names != EXPECTED_TEST_BINS:
        missing = sorted(EXPECTED_TEST_BINS - marked_names)
        added = sorted(marked_names - EXPECTED_TEST_BINS)
        if missing:
            errors.append(f"known test-bearing bins lost markers: {', '.join(missing)}")
        if added:
            errors.append(f"unexpected test-bearing bins: {', '.join(added)}")

    if errors:
        print(f"lint-discover-bin-tests: {len(errors)} error(s)", file=sys.stderr)
        for error in errors:
            print(f"  {error}", file=sys.stderr)
        return 1
    print(
        "lint-discover-bin-tests: clean "
        f"({len(source_paths)} bins; {len(marked_names)} test-bearing, "
        f"{len(source_paths) - len(marked_names)} test-disabled)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
