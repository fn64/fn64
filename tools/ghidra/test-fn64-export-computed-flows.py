#!/usr/bin/env python3
"""Focused source and digest-contract tests for Fn64ExportComputedFlows.java."""

from __future__ import annotations

import hashlib
from pathlib import Path
import struct
import unittest


SOURCE = Path(__file__).with_name("Fn64ExportComputedFlows.java")


def put_string(digest: "hashlib._Hash", value: str) -> None:
    encoded = value.encode()
    digest.update(struct.pack("<Q", len(encoded)))
    digest.update(encoded)


def put_address(digest: "hashlib._Hash", bank: str, pc: int) -> None:
    put_string(digest, bank)
    digest.update(struct.pack("<I", pc))


def claims_digest(
    bank: str, flows: list[tuple[int, bool, list[int]]]
) -> str:
    digest = hashlib.sha256(b"fn64.tool-adapter.claim-records.v1\0")
    digest.update(struct.pack("<Q", len(flows)))
    for sequence, (site, via_call, targets) in enumerate(flows):
        digest.update(struct.pack("<Q", sequence))
        put_string(digest, f"ghidra:computed-flow:{bank}:{site:08x}")
        digest.update(b"\x07")
        put_address(digest, bank, site)
        digest.update(bytes([via_call]))
        digest.update(struct.pack("<Q", len(targets)))
        for target in targets:
            put_address(digest, bank, target)
        digest.update(b"\x00")  # completeness = unknown
    return digest.hexdigest()


class ComputedFlowExporterSourceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.source = SOURCE.read_text(encoding="utf-8")

    def test_emits_strict_candidate_only_schema_v3(self) -> None:
        self.assertIn('"schema_version\\\":3', self.source)
        self.assertIn('\\"role\\":\\"control_flow_candidates\\"', self.source)
        self.assertIn('\\"type\\":\\"computed_control_flow\\"', self.source)
        self.assertIn('\\"completeness\\":\\"unknown\\"', self.source)
        self.assertNotIn('"exhaustive"', self.source)

    def test_selects_only_computed_calls_and_jumps_and_excludes_returns(self) -> None:
        self.assertIn("flowType.isComputed()", self.source)
        self.assertIn("flowType.isCall()", self.source)
        self.assertIn("flowType.isJump()", self.source)
        self.assertIn("isOrdinaryReturn(instruction, flowType)", self.source)
        self.assertIn('getMnemonicString().equalsIgnoreCase("jr")', self.source)
        self.assertIn('register.getName().equalsIgnoreCase("ra")', self.source)

    def test_only_nonfallthrough_flow_references_become_targets(self) -> None:
        self.assertIn("instruction.getReferencesFrom()", self.source)
        self.assertIn("referenceType.isFlow()", self.source)
        self.assertIn("referenceType.isComputed()", self.source)
        self.assertIn("referenceType.isFallthrough()", self.source)
        self.assertIn("targetOffset < vaStart || targetOffset >= vaEnd", self.source)
        self.assertIn("ignored_out_of_bank_flow_references=", self.source)
        self.assertIn("TreeSet<Long> targets", self.source)

    def test_bank_bytes_and_program_identity_fail_closed(self) -> None:
        for guard in (
            "wrong program",
            "invalid bank interval",
            "bank interval is not in the default address space",
            "bank interval is not one readable default-space block",
            "mapped bank digest mismatch",
            "computed-flow instruction is not one aligned MIPS word",
            "computed-flow target is not word-aligned",
            "incompatible duplicate computed-flow site",
        ):
            with self.subTest(guard=guard):
                self.assertIn(guard, self.source)

    def test_digest_model_is_order_and_semantics_sensitive(self) -> None:
        canonical = [
            (0x80001010, False, []),
            (0x80001020, True, [0x80001030, 0x80001040]),
        ]
        self.assertEqual(
            claims_digest("bank-a", canonical),
            claims_digest("bank-a", list(canonical)),
        )
        self.assertNotEqual(
            claims_digest("bank-a", canonical),
            claims_digest("bank-a", list(reversed(canonical))),
        )
        changed_call = list(canonical)
        changed_call[1] = (0x80001020, False, [0x80001030, 0x80001040])
        self.assertNotEqual(
            claims_digest("bank-a", canonical), claims_digest("bank-a", changed_call)
        )
        changed_targets = list(canonical)
        changed_targets[1] = (0x80001020, True, [0x80001030])
        self.assertNotEqual(
            claims_digest("bank-a", canonical), claims_digest("bank-a", changed_targets)
        )


if __name__ == "__main__":
    unittest.main()
