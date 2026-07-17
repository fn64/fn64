#!/usr/bin/env python3
"""Reject hand-written N64Recomp RDRAM lane mapping outside its owner.

RDRAM storage is native-endian by 32-bit word while guest byte and halfword
addresses use the generated ABI's ^3/^2 lane mapping. That rule belongs only
in fn64-runtime::rdram. Host/device adapters use RdramView, RdramViewMut, or
RdramPtr so a new boundary cannot silently choose a different layout.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SCAN_ROOTS = (
    "crates/fn64-abi/src",
    "crates/fn64-render/src",
    "crates/fn64-render-rt64/src",
    "crates/fn64-runtime/src",
    "crates/fn64-shell/src",
    "examples/oot-boot/src",
    "examples/wm2000-boot/src",
)
CANONICAL = {Path("crates/fn64-runtime/src/rdram.rs")}
TEST_CFG = re.compile(r"(?m)^\s*#\[cfg\(test\)\]")
TEST_MODULE = re.compile(r"\bmod\s+tests\s*\{")
CHECKS = (
    ("manual ^2/^3 lane mapping", re.compile(r"\^\s*[23](?![0-9])")),
    (
        "raw RDRAM indexed write",
        re.compile(r"\brdram\s*\[[^\]]+\]\s*(?:=|\.copy_from_slice\s*\()"),
    ),
    (
        "raw RDRAM pointer write",
        re.compile(r"\*\s*rdram\.add\s*\([^)]*\)\s*="),
    ),
)


def production_source(text: str) -> str:
    for cfg in TEST_CFG.finditer(text):
        if TEST_MODULE.search(text[cfg.end() : cfg.end() + 800]):
            return text[: cfg.start()]
    return text


def find_failures(relative: Path, source: str) -> list[str]:
    failures: list[str] = []
    for number, line in enumerate(production_source(source).splitlines(), 1):
        code = line.split("//", 1)[0]
        for label, pattern in CHECKS:
            if pattern.search(code):
                failures.append(f"{relative}:{number}: {label}: {line.strip()}")
    return failures


def selftest() -> int:
    bad = """
fn old_writer(rdram: &mut [u8], dst: usize, px: u16) {
    let [hi, lo] = px.to_be_bytes();
    rdram[dst] = hi;
    rdram[dst + 1] = lo;
}
"""
    manual = "fn old_reader(rdram: &[u8], off: usize) -> u8 { rdram[off ^ 3] }"
    good = "fn writer(view: &mut RdramViewMut<'_>, addr: RdramAddr) { view.write_u16(addr, 1); }"
    hidden_test = """
fn production() {}
#[cfg(test)]
mod tests { fn fixture(rdram: &mut [u8]) { rdram[0 ^ 3] = 1; } }
"""
    assert find_failures(Path("old_writer.rs"), bad), "flat raw writer must fail"
    assert find_failures(Path("old_reader.rs"), manual), "manual lane XOR must fail"
    assert not find_failures(Path("typed.rs"), good), "typed view must pass"
    assert not find_failures(Path("tests.rs"), hidden_test), "fixture-only code is out of scope"
    print("RDRAM layout boundary lint selftest: 4/4")
    return 0


def main() -> int:
    if sys.argv[1:] == ["--selftest"]:
        return selftest()
    if sys.argv[1:]:
        print("usage: scripts/lint-rdram-layout.py [--selftest]", file=sys.stderr)
        return 2

    failures: list[str] = []
    for root_name in SCAN_ROOTS:
        for path in sorted((ROOT / root_name).rglob("*.rs")):
            relative = path.relative_to(ROOT)
            if relative in CANONICAL:
                continue
            failures.extend(find_failures(relative, path.read_text()))

    if failures:
        print("RDRAM layout boundary violations:", file=sys.stderr)
        print("\n".join(failures), file=sys.stderr)
        print(
            "Use fn64_runtime::{RdramView, RdramViewMut, RdramPtr}; "
            "the lane mapping is owned by fn64-runtime/src/rdram.rs.",
            file=sys.stderr,
        )
        return 1

    print("RDRAM layout boundary lint: clean")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
