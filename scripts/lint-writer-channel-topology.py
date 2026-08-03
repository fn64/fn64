#!/usr/bin/env python3
"""Lock the typed PI/SI/SP writer-attribution topology.

This is a structural drift gate, not a writer-channel completion validator.
It proves that the runtime's device fabric selects a typed DMA producer and
that the canonical ABI adapter maps every such producer to its matching
recompiler notification. It deliberately cannot prove that an installed
program model reaches only the canonical ABI adapter.
"""

from __future__ import annotations

import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
ROM = Path("crates/fn64-runtime/src/rom.rs")
DEVICE = Path("crates/fn64-runtime/src/device.rs")
PI = Path("crates/fn64-abi/src/pi.rs")


def count(source: str, needle: str) -> int:
    return source.count(needle)


def production_source(source: str) -> str:
    test_module = source.find("#[cfg(test)]")
    return source if test_module < 0 else source[:test_module]


def audit(sources: dict[Path, str]) -> list[str]:
    failures: list[str] = []
    rom = production_source(sources[ROM])
    device = production_source(sources[DEVICE])
    pi = production_source(sources[PI])

    expected_impls = (
        "impl DmaMemory for crate::rdram::Rdram {",
        "impl DmaMemory for crate::rdram::RdramViewMut<'_> {",
        "impl DmaMemory for ProcessDmaMemory<'_> {",
    )
    for implementation in expected_impls:
        if count(rom, implementation) != 1:
            failures.append(
                f"{ROM}: expected exactly one sealed implementation `{implementation}`"
            )
    if count(rom, "impl DmaMemory for ") != len(expected_impls):
        failures.append(
            f"{ROM}: DmaMemory implementation denominator changed; audit the new owner"
        )

    device_writers = {
        "Pi": 0,
        "Si": 1,
        "Sp": 1,
    }
    for channel, expected in device_writers.items():
        needle = f"DmaWriterChannel::{channel}"
        actual = count(device, needle)
        if actual != expected:
            failures.append(
                f"{DEVICE}: expected {expected} production `{needle}` selections, found {actual}"
            )

    mappings = {
        "Pi": "fn64_recomp_rs::notify_pi_dma_write",
        "Si": "fn64_recomp_rs::notify_si_dma_write",
        "Sp": "fn64_recomp_rs::notify_sp_dma_write",
    }
    for channel, notification in mappings.items():
        arm = f"fn64_runtime::DmaWriterChannel::{channel} => {notification}"
        if count(pi, arm) != 1:
            failures.append(f"{PI}: missing or duplicate exact producer mapping `{arm}`")

    if count(pi, "fn64_runtime::ProcessDmaMemory::from_raw_parts(") < 2:
        failures.append(
            f"{PI}: canonical live device paths no longer visibly use ProcessDmaMemory"
        )
    return failures


def selftest() -> int:
    good = {
        ROM: "\n".join(
            (
                "impl DmaMemory for crate::rdram::Rdram {",
                "impl DmaMemory for crate::rdram::RdramViewMut<'_> {",
                "impl DmaMemory for ProcessDmaMemory<'_> {",
            )
        ),
        DEVICE: "DmaWriterChannel::Si\nDmaWriterChannel::Sp",
        PI: "\n".join(
            (
                "fn64_runtime::DmaWriterChannel::Pi => fn64_recomp_rs::notify_pi_dma_write",
                "fn64_runtime::DmaWriterChannel::Si => fn64_recomp_rs::notify_si_dma_write",
                "fn64_runtime::DmaWriterChannel::Sp => fn64_recomp_rs::notify_sp_dma_write",
                "fn64_runtime::ProcessDmaMemory::from_raw_parts(",
                "fn64_runtime::ProcessDmaMemory::from_raw_parts(",
            )
        ),
    }
    assert not audit(good), "known-good topology must pass"
    escaped = dict(good)
    escaped[ROM] += "\nimpl DmaMemory for ForeignMemory {"
    assert audit(escaped), "a new DmaMemory implementation must fail"
    erased = dict(good)
    erased[PI] = erased[PI].replace(
        "fn64_runtime::DmaWriterChannel::Si => fn64_recomp_rs::notify_si_dma_write",
        "fn64_runtime::DmaWriterChannel::Si => fn64_recomp_rs::notify_pi_dma_write",
    )
    assert audit(erased), "producer erasure must fail"
    bypass = dict(good)
    bypass[DEVICE] += "\nDmaWriterChannel::Si"
    assert audit(bypass), "a second SI write site must fail"
    print("writer-channel topology lint selftest: 4/4")
    return 0


def main() -> int:
    if sys.argv[1:] == ["--selftest"]:
        return selftest()
    if sys.argv[1:]:
        print(
            "usage: scripts/lint-writer-channel-topology.py [--selftest]",
            file=sys.stderr,
        )
        return 2

    sources = {
        path: (ROOT / path).read_text()
        for path in (ROM, DEVICE, PI)
    }
    failures = audit(sources)
    if failures:
        print("writer-channel topology violations:", file=sys.stderr)
        print("\n".join(failures), file=sys.stderr)
        print(
            "Audit the fixed writer denominator and canonical ABI attribution before changing this gate.",
            file=sys.stderr,
        )
        return 1
    print("writer-channel topology lint: clean (PI/SI/SP typed producer mapping)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
