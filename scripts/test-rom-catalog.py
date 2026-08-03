#!/usr/bin/env python3
"""Regression tests for rom-catalog.py.

Structural tests are ROM-free and synthesize their own headers and code. The
corpus assertions are keyed by normalized ROM digest, not filename, and skip
when FN64_ROM_CORPUS_DIR is unset, so this file never depends on ROM bytes
being present.
"""

from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import struct
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().with_name("rom-catalog.py")
SPEC = importlib.util.spec_from_file_location("rom_catalog", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
CATALOG = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = CATALOG
SPEC.loader.exec_module(CATALOG)


def synthetic_rom(
    *,
    entry_point: int = 0x8000_0400,
    name: bytes = b"TEST CART",
    cartridge_id: bytes = b"NTSP",
    body: bytes = b"",
) -> bytes:
    """A minimal well-formed big-endian ROM: header, IPL3 filler, boot copy."""
    header = bytearray(CATALOG.IPL3_ROM_END)
    struct.pack_into(">I", header, 0x00, CATALOG.MAGIC_Z64)
    struct.pack_into(">I", header, 0x08, entry_point)
    header[0x20 : 0x20 + len(name)] = name
    header[0x3B:0x3F] = cartridge_id
    boot = bytearray(CATALOG.BOOT_COPY_SIZE)
    boot[: len(body)] = body
    return bytes(header) + bytes(boot)


def words(*encoded: int) -> bytes:
    return b"".join(struct.pack(">I", word) for word in encoded)


class ByteOrderTests(unittest.TestCase):
    def test_z64_passes_through_unchanged(self) -> None:
        rom = synthetic_rom()
        normalized, order = CATALOG.normalize_to_big_endian(rom)
        self.assertEqual(order, "z64")
        self.assertEqual(normalized, rom)

    def test_n64_and_v64_normalize_to_the_same_bytes(self) -> None:
        rom = synthetic_rom()
        swapped_words = b"".join(
            rom[index : index + 4][::-1] for index in range(0, len(rom), 4)
        )
        swapped_halves = b"".join(
            rom[index : index + 2][::-1] for index in range(0, len(rom), 2)
        )
        for encoded, expected_order in ((swapped_words, "n64"), (swapped_halves, "v64")):
            normalized, order = CATALOG.normalize_to_big_endian(encoded)
            self.assertEqual(order, expected_order)
            self.assertEqual(normalized, rom)

    def test_unknown_magic_is_loud(self) -> None:
        with self.assertRaises(CATALOG.CatalogError):
            CATALOG.normalize_to_big_endian(b"\x00" * CATALOG.IPL3_ROM_END)


class HeaderTests(unittest.TestCase):
    def test_header_fields_decode_at_documented_offsets(self) -> None:
        rom = synthetic_rom(entry_point=0x8024_6000, name=b"SUPER MARIO 64")
        fields = CATALOG.read_header_fields(rom)
        self.assertEqual(fields["internal_name"], "SUPER MARIO 64")
        self.assertEqual(fields["entry_point"], 0x8024_6000)
        self.assertEqual(fields["region"], "P")
        self.assertEqual(fields["cartridge_code"], "TS")


class CodeRunTests(unittest.TestCase):
    def test_short_runs_are_not_admitted(self) -> None:
        # One `addu` short of the floor must contribute nothing, otherwise the
        # hazard census would count data as instructions.
        short = [0x0000_0021] * (CATALOG.MIN_CODE_RUN_WORDS - 1) + [0xFFFF_FFFF]
        self.assertEqual(CATALOG.code_run_spans(short), [])

    def test_run_at_the_floor_is_admitted(self) -> None:
        exact = [0x0000_0021] * CATALOG.MIN_CODE_RUN_WORDS + [0xFFFF_FFFF]
        self.assertEqual(
            CATALOG.code_run_spans(exact), [(0, CATALOG.MIN_CODE_RUN_WORDS)]
        )

    def test_trailing_run_is_closed(self) -> None:
        trailing = [0xFFFF_FFFF] + [0x0000_0021] * CATALOG.MIN_CODE_RUN_WORDS
        self.assertEqual(
            CATALOG.code_run_spans(trailing),
            [(1, 1 + CATALOG.MIN_CODE_RUN_WORDS)],
        )

    def test_hazards_are_counted_only_inside_code_runs(self) -> None:
        # An isolated `lwl` surrounded by reserved encodings is data, not an
        # unaligned load. This is the specific bug the run floor guards: a
        # whole-bank census reports atomics that N64 titles never execute.
        lwl = 0x8880_0000
        stray = words(0xFFFF_FFFF, lwl, 0xFFFF_FFFF)
        rom = synthetic_rom(body=stray)
        self.assertEqual(CATALOG.measure_boot_bank(rom)["unaligned_mem"], 0)

        run = words(*([0x0000_0021] * CATALOG.MIN_CODE_RUN_WORDS), lwl)
        rom = synthetic_rom(body=run)
        self.assertEqual(CATALOG.measure_boot_bank(rom)["unaligned_mem"], 1)


class BootBankTests(unittest.TestCase):
    def test_counts_targets_returns_and_prologues(self) -> None:
        jal = 0x0C00_0000 | ((0x8000_1000 >> 2) & 0x03FF_FFFF)
        prologue = 0x27BD_FFE8
        body = words(prologue, jal, jal, CATALOG.WORD_JR_RA)
        measures = CATALOG.measure_boot_bank(synthetic_rom(body=body))
        self.assertEqual(measures["distinct_jal_targets"], 1)
        self.assertEqual(measures["jr_ra_count"], 1)
        self.assertEqual(measures["stack_prologue_count"], 1)
        self.assertEqual(measures["loader_stub_ratio"], 1.0)

    def test_zero_returns_reports_ratio_without_dividing_by_zero(self) -> None:
        jal = 0x0C00_0000 | ((0x8000_1000 >> 2) & 0x03FF_FFFF)
        measures = CATALOG.measure_boot_bank(synthetic_rom(body=words(jal)))
        self.assertEqual(measures["jr_ra_count"], 0)
        self.assertEqual(measures["loader_stub_ratio"], 1.0)


class StableIdTests(unittest.TestCase):
    def test_ids_are_path_free_and_lowercase(self) -> None:
        self.assertEqual(
            CATALOG.stable_id(Path("/roms/007 - GoldenEye (Europe).z64")),
            "007-goldeneye-europe",
        )

    def test_non_alphanumeric_only_name_still_yields_an_id(self) -> None:
        self.assertEqual(CATALOG.stable_id(Path("/roms/!!!.z64")), "rom")


DAT_SAMPLE = """clrmamepro (
\tname "Nintendo - Nintendo 64"
)

game (
\tcomment "Test Game (USA)"
\tdeveloper "Iguana Entertainment"
\trom ( crc DEADBEEF )
)

game (
\tcomment "Other Game (USA)"
\tdeveloper "Rareware"
\trom ( crc 12345678 )
)
"""


class DatJoinTests(unittest.TestCase):
    def test_records_parse_and_key_on_lowercase_crc(self) -> None:
        table = CATALOG.parse_dat(DAT_SAMPLE, "developer")
        self.assertEqual(table["deadbeef"], ("Iguana Entertainment", "Test Game (USA)"))
        self.assertEqual(table["12345678"][0], "Rareware")

    def test_a_matched_rom_gains_metadata(self) -> None:
        tables = {field: CATALOG.parse_dat(DAT_SAMPLE, "developer") for field in CATALOG.DAT_FIELDS}
        record = {"file_crc32": "deadbeef"}
        CATALOG.join_dat(record, tables)
        self.assertTrue(record["dat_match"])
        self.assertEqual(record["developer"], "Iguana Entertainment")
        self.assertEqual(record["dat_name"], "Test Game (USA)")
        # `releaseyear` is renamed to the catalog's field name.
        self.assertIn("release_year", record)
        self.assertNotIn("releaseyear", record)

    def test_an_unmatched_rom_is_an_explicit_miss_not_a_guess(self) -> None:
        # No-Intro does not catalog hacks, translations or prototypes. An
        # absent row must stay empty rather than borrow a neighbour's values.
        tables = {field: CATALOG.parse_dat(DAT_SAMPLE, "developer") for field in CATALOG.DAT_FIELDS}
        record = {"file_crc32": "ffffffff"}
        CATALOG.join_dat(record, tables)
        self.assertFalse(record["dat_match"])
        self.assertEqual(record["developer"], "")
        self.assertEqual(record["dat_name"], "")

    def test_a_missing_dat_file_is_loud(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaises(CATALOG.CatalogError):
                CATALOG.load_dat_directory(Path(directory))


class OutputTests(unittest.TestCase):
    def test_relative_and_dotdot_destinations_are_refused(self) -> None:
        for candidate in ("relative.jsonl", "/tmp/../etc/out.jsonl"):
            with self.assertRaises(CATALOG.CatalogError):
                CATALOG.validate_output_destination(candidate)

    def test_existing_destination_is_never_overwritten(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory).resolve() / "catalog.jsonl"
            path.write_text("existing")
            with self.assertRaises(CATALOG.CatalogError):
                CATALOG.validate_output_destination(str(path))

    def test_publish_writes_canonical_jsonl(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory).resolve() / "catalog.jsonl"
            CATALOG.publish_records(path, [{"b": 2, "a": 1}])
            self.assertEqual(path.read_bytes(), b'{"a":1,"b":2}\n')


CORPUS_DIR = os.environ.get("FN64_ROM_CORPUS_DIR")


@unittest.skipUnless(CORPUS_DIR, "set FN64_ROM_CORPUS_DIR to run corpus assertions")
class CorpusTests(unittest.TestCase):
    """Pinned measurements, keyed by normalized ROM digest rather than name.

    These derive from a fixed decode of fixed bytes, so a change here is a real
    regression in the measurement code, not corpus drift.
    """

    # normalized_rom_sha256 -> expected subset of the catalog record.
    EXPECTED = {
        # Super Mario 64 (USA): resident boot-bank code, the graded control.
        "17ce077343c6133f8c9f2d6d6d9a4ab62c8cd2aa57c40aea1f490b4c8bb21d91": {
            "distinct_jal_targets": 2532,
            "jr_ra_count": 4314,
        },
    }

    @classmethod
    def setUpClass(cls) -> None:
        assert CORPUS_DIR is not None
        cls.records = {}
        for rom_path in CATALOG.discover_roms(Path(CORPUS_DIR)):
            record = CATALOG.catalog_rom(rom_path)
            cls.records[record["normalized_rom_sha256"]] = record

    def test_every_rom_catalogs_without_error(self) -> None:
        self.assertGreater(len(self.records), 0)

    def test_pinned_measurements_hold(self) -> None:
        for digest, expected in self.EXPECTED.items():
            record = self.records.get(digest)
            if record is None:
                self.skipTest(f"corpus does not contain {digest[:16]}")
            for field, value in expected.items():
                self.assertEqual(record[field], value, f"{field} for {digest[:16]}")

    def test_resident_and_stub_classes_separate(self) -> None:
        """The loader_stub_ratio split is the catalog's bucketing claim."""
        by_name = {record["internal_name"]: record for record in self.records.values()}
        for name, ceiling in (("SUPER MARIO 64", 2.0), ("GOLDENEYE", 2.0)):
            if name in by_name:
                self.assertLess(by_name[name]["loader_stub_ratio"], ceiling, name)
        for name, floor in (("Banjo-Kazooie", 10.0), ("DONKEY KONG 64", 10.0)):
            if name in by_name:
                self.assertGreater(by_name[name]["loader_stub_ratio"], floor, name)


if __name__ == "__main__":
    unittest.main()
