#!/usr/bin/env python3
"""Focused source-contract tests for Fn64ExportLoaderComparison.java."""

from __future__ import annotations

import hashlib
import pathlib
import struct
import unittest


SOURCE = pathlib.Path(__file__).with_name("Fn64ExportLoaderComparison.java")
DOMAIN = b"fn64.ghidra-bank-function-inventory.v1\0"
CHUNK_SIZE = 64 * 1024


def inventory_digest(functions: list[tuple[int, list[tuple[int, int]]]]) -> str:
    """Independent model of the documented canonical inventory digest."""
    canonical = sorted((entry, sorted(ranges)) for entry, ranges in functions)
    digest = hashlib.sha256(DOMAIN)
    digest.update(struct.pack("<Q", len(canonical)))
    for entry, ranges in canonical:
        digest.update(struct.pack("<I", entry))
        digest.update(struct.pack("<Q", len(ranges)))
        for start, end in ranges:
            digest.update(struct.pack("<II", start, end))
    return digest.hexdigest()


def mapped_digest_model(
    context_start: int,
    context_end: int,
    bank_start: int,
    bank_end: int,
    blocks: list[tuple[int, bytes, tuple[bool, bool, bool, bool]]],
) -> tuple[str, str, int, list[tuple[int, int, int, int, tuple[bool, bool, bool, bool]]]]:
    """Independent model of adjacent-block traversal and bank slicing."""
    context_digest = hashlib.sha256()
    bank_digest = hashlib.sha256()
    bank_consumed = 0
    cursor = context_start
    geometry = []
    for block_start, block_bytes, permissions in sorted(blocks):
        block_end = block_start + len(block_bytes)
        overlap_start = max(block_start, context_start)
        overlap_end = min(block_end, context_end)
        if overlap_start >= overlap_end:
            continue
        if overlap_start != cursor:
            raise ValueError("synthetic mapping has a gap or overlap")
        geometry.append((block_start, block_end, overlap_start, overlap_end, permissions))
        chunk_start = overlap_start
        while chunk_start < overlap_end:
            chunk_end = min(chunk_start + CHUNK_SIZE, overlap_end)
            buffer_start = chunk_start - block_start
            buffer_end = chunk_end - block_start
            chunk = block_bytes[buffer_start:buffer_end]
            context_digest.update(chunk)
            bank_overlap_start = max(chunk_start, bank_start)
            bank_overlap_end = min(chunk_end, bank_end)
            if bank_overlap_start < bank_overlap_end:
                start = bank_overlap_start - chunk_start
                end = bank_overlap_end - chunk_start
                bank_digest.update(chunk[start:end])
                bank_consumed += end - start
            chunk_start = chunk_end
        cursor = overlap_end
    if cursor != context_end:
        raise ValueError("synthetic mapping does not cover the context")
    return (
        context_digest.hexdigest(),
        bank_digest.hexdigest(),
        bank_consumed,
        geometry,
    )


class ExportLoaderComparisonSourceTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.source = SOURCE.read_text(encoding="utf-8")

    def test_common_schema_and_runner_supplied_provenance(self) -> None:
        self.assertIn('args.length != 14', self.source)
        self.assertIn('"binary-loader"', self.source)
        self.assertIn('"n64loaderwv"', self.source)
        self.assertIn('phase.equals("pre")', self.source)
        self.assertIn('phase.equals("post")', self.source)
        self.assertIn('\\"schema\\":\\"fn64.ghidra-bank-function-inventory\\"', self.source)
        self.assertIn('\\"schema_version\\":4', self.source)
        self.assertIn('\\"source_sha256\\":\\"', self.source)

    def test_output_is_loader_neutral_and_preserves_exact_ranges(self) -> None:
        self.assertIn('\\"entry\\":', self.source)
        self.assertIn('\\"body_ranges\\":[', self.source)
        self.assertIn('\\"va_start\\":', self.source)
        self.assertIn('\\"va_end\\":', self.source)
        self.assertNotIn("function.getName()", self.source)
        self.assertNotIn("block.getName()", self.source)
        self.assertIn('\\"memory_blocks\\":[', self.source)
        self.assertIn('\\"entry_points\\":[', self.source)
        self.assertIn("getExternalEntryPointIterator()", self.source)
        self.assertIn("fn64.ghidra-bank-entry-points.v1", self.source)
        self.assertIn('\\"rejected_functions\\":[', self.source)
        self.assertIn("fn64.ghidra-bank-rejected-functions.v1", self.source)
        self.assertIn('"non_word_body_range"', self.source)
        self.assertIn('\\"overlap_start\\":', self.source)
        self.assertIn('\\"read\\":', self.source)
        self.assertIn('\\"write\\":', self.source)
        self.assertIn('\\"execute\\":', self.source)
        self.assertIn('\\"initialized\\":', self.source)
        self.assertIn("Math.min(length - consumed, bytesInBlock)", self.source)

    def test_selected_bodies_fail_closed(self) -> None:
        required_guards = (
            "function body crosses address spaces",
            "function body violates the supplied bank mapping",
            "function body ranges overlap",
            "function body does not contain its entry",
            "duplicate function entry in bank",
            "mapped bank digest mismatch",
            "mapped context digest mismatch",
            "bank interval was not fully covered by the context",
        )
        for guard in required_guards:
            with self.subTest(guard=guard):
                self.assertIn(guard, self.source)

    def test_canonical_digest_is_order_independent_but_gap_sensitive(self) -> None:
        first = [
            (0x80001020, [(0x80001040, 0x80001048), (0x80001020, 0x80001030)]),
            (0x80001000, [(0x80001000, 0x80001010)]),
        ]
        reordered = list(reversed([(entry, list(reversed(ranges))) for entry, ranges in first]))
        widened = [
            (0x80001020, [(0x80001020, 0x80001048)]),
            (0x80001000, [(0x80001000, 0x80001010)]),
        ]
        self.assertEqual(inventory_digest(first), inventory_digest(reordered))
        self.assertNotEqual(inventory_digest(first), inventory_digest(widened))
        self.assertIn("ranges.sort(Comparator.comparingLong(BodyRange::start)", self.source)
        self.assertIn("result.sort(Comparator.comparingLong(FunctionBody::entry))", self.source)
        self.assertIn("putU64(digest, function.ranges().size())", self.source)

    def test_context_and_bank_are_hashed_in_one_mapping_pass(self) -> None:
        self.assertIn("contextDigest.update(buffer, 0, chunkLength)", self.source)
        self.assertIn("bankDigest.update(buffer, bufferOffset, bankLength)", self.source)
        self.assertIn("bankConsumed = Math.addExact(bankConsumed, bankLength)", self.source)
        self.assertEqual(self.source.count("while (consumed < length)"), 1)

    def test_synthetic_bank_crosses_chunk_and_adjacent_block_boundaries(self) -> None:
        context_start = 0x80000000
        split = context_start + CHUNK_SIZE + 0x10
        context_end = split + 0x8030
        bank_start = context_start + CHUNK_SIZE - 8
        bank_end = split + 0x24
        first = bytes((index * 17 + 3) & 0xFF for index in range(split - context_start))
        second = bytes((index * 29 + 5) & 0xFF for index in range(context_end - split + 0x20))
        permissions = (True, True, True, True)
        blocks = [
            (split, second, permissions),
            (context_start, first, permissions),
        ]

        context_sha, bank_sha, consumed, geometry = mapped_digest_model(
            context_start, context_end, bank_start, bank_end, blocks
        )
        expected_context = first + second[: context_end - split]
        expected_bank = expected_context[
            bank_start - context_start : bank_end - context_start
        ]
        self.assertEqual(context_sha, hashlib.sha256(expected_context).hexdigest())
        self.assertEqual(bank_sha, hashlib.sha256(expected_bank).hexdigest())
        self.assertEqual(consumed, bank_end - bank_start)
        self.assertEqual(
            geometry,
            [
                (context_start, split, context_start, split, permissions),
                (split, split + len(second), split, context_end, permissions),
            ],
        )
        self.assertEqual(
            mapped_digest_model(
                context_start, context_end, bank_start, bank_end, list(reversed(blocks))
            ),
            (context_sha, bank_sha, consumed, geometry),
        )

    def test_unrelated_default_space_functions_are_filtered_not_rejected(self) -> None:
        entry_filter = "if (entry < vaStart || entry >= vaEnd) {\n                continue;"
        self.assertIn(entry_filter, self.source)
        self.assertNotIn("out-of-bank function entry", self.source)
        filter_offset = self.source.index(entry_filter)
        space_guard = "function entry in bank uses a non-default address space"
        self.assertIn(space_guard, self.source)
        self.assertLess(filter_offset, self.source.index(space_guard))


if __name__ == "__main__":
    unittest.main()
