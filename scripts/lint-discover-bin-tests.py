#!/usr/bin/env python3
"""Keep fn64-discover's subcommand modules and their unit tests explicit.

fn64-discover used to be 51 separate `[[bin]]` targets under `src/bin/`,
each linking its own copy of the workspace (Task 2.3, see
docs/plans/CLEANUP-2026-09.md). It is now one binary (`src/main.rs`) with
one `commands::<name>` module per former bin, dispatched by a clap
subcommand. This lint keeps that module list and `src/main.rs`'s
`Command` enum in exact 1:1 correspondence, and keeps each module's
`#[cfg(test)]`/`#[test]` presence matching a known, explicit set (so a
module silently gaining or losing its unit tests gets noticed).
"""

import re
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
CRATE = ROOT / "crates/fn64-discover"
COMMANDS_DIR = CRATE / "src/commands"
COMMANDS_MOD = COMMANDS_DIR / "mod.rs"
MAIN_RS = CRATE / "src/main.rs"

TEST_MARKER = re.compile(r"(?m)^\s*#\s*\[\s*(?:cfg\s*\(\s*test\s*\)|test)\s*\]")
MOD_DECL = re.compile(r"(?m)^pub mod (\w+);")
VARIANT_DECL = re.compile(r"(?m)^\s{4}(\w+)\(PassthroughArgs\),")

# Every subcommand module known to carry a unit-test module today. A module
# moving into or out of this set is a real change worth a reviewer's eye,
# so it must be reflected here explicitly rather than inferred silently.
EXPECTED_TEST_MODULES = {
    "attribute_known_functions",
    "classify_callerless",
    "compare_computed_flows",
    "corpus_index",
    "fn64_discover_run",
    "gate_d1_oot_overlays",
    "gate_owners_overlays",
    "gate_recompiler_lint",
    "gate_rom_rebuild",
    "gate_rom_recompile",
    "ingest_tool_claims",
    "produce_snapshot_workspace",
    "rom_identity",
    "run_wm_writer_audit",
    "validate_executable_image_group",
}


def main() -> int:
    errors: list[str] = []

    module_files = {
        path.stem
        for path in sorted(COMMANDS_DIR.glob("*.rs"))
        if path.name != "mod.rs"
    }
    declared_modules = set(MOD_DECL.findall(COMMANDS_MOD.read_text()))

    for name in sorted(module_files - declared_modules):
        errors.append(f"src/commands/{name}.rs exists but is not `pub mod`-declared in commands/mod.rs")
    for name in sorted(declared_modules - module_files):
        errors.append(f"commands/mod.rs declares `{name}` but src/commands/{name}.rs does not exist")

    main_source = MAIN_RS.read_text()

    # Convert each PascalCase Command variant name to the snake_case module
    # name it must dispatch to (e.g. GateB1 -> gate_b1, Run -> run handled
    # separately since it maps to fn64_discover_run, not `run`).
    def variant_to_module(variant: str) -> str:
        if variant == "Run":
            return "fn64_discover_run"
        snake = re.sub(r"(?<!^)(?=[A-Z])", "_", variant).lower()
        return snake

    variant_names = VARIANT_DECL.findall(main_source)
    if len(variant_names) != 51:
        errors.append(f"expected 51 Command variants, found {len(variant_names)}")

    expected_modules_from_variants = {variant_to_module(v) for v in variant_names}
    for name in sorted(expected_modules_from_variants - declared_modules):
        errors.append(f"Command variant expects module `{name}` but it is not declared in commands/mod.rs")
    for name in sorted(declared_modules - expected_modules_from_variants):
        errors.append(f"commands/mod.rs declares `{name}` with no matching Command variant")

    test_bearing: set[str] = set()
    for name in sorted(module_files):
        path = COMMANDS_DIR / f"{name}.rs"
        if TEST_MARKER.search(path.read_text()):
            test_bearing.add(name)

    if test_bearing != EXPECTED_TEST_MODULES:
        missing = sorted(EXPECTED_TEST_MODULES - test_bearing)
        added = sorted(test_bearing - EXPECTED_TEST_MODULES)
        if missing:
            errors.append(f"known test-bearing modules lost their unit tests: {', '.join(missing)}")
        if added:
            errors.append(
                "modules gained unit tests without updating EXPECTED_TEST_MODULES in this "
                f"lint: {', '.join(added)}"
            )

    if errors:
        print(f"lint-discover-bin-tests: {len(errors)} error(s)", file=sys.stderr)
        for error in errors:
            print(f"  {error}", file=sys.stderr)
        return 1
    print(
        "lint-discover-bin-tests: clean "
        f"({len(module_files)} subcommand modules; {len(test_bearing)} test-bearing, "
        f"{len(module_files) - len(test_bearing)} test-free)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
