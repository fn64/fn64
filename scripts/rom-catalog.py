#!/usr/bin/env python3
"""Catalog a private local ROM corpus as canonical JSONL, one record per ROM.

Every field is measured from ROM bytes; nothing is inferred from a filename.
ROMs are private local capabilities and their bytes never reach the output --
records carry digests, header fields, and aggregate structural measures only.

Three measurement tiers:
  1. cartridge identity from the 0x1000-byte header region;
  2. boot-bank structure, decoding ROM [0x1000, 0x101000) as MIPS words;
  3. recompiler-hazard census, counted only inside long code runs.

Tier 3 deliberately refuses to count hazards over the raw boot bank: most of a
boot bank is frequently data, and decoding data as instructions reports
thousands of atomics that N64 titles never execute.
"""

from __future__ import annotations

import argparse
import collections
import hashlib
import json
import math
import os
import re
import secrets
import stat
import struct
import sys
import zlib
from pathlib import Path
from typing import Any, Iterable


CATALOG_SCHEMA = "fn64.rom-catalog.v1"

# Header magics, mirrored from crates/fn64-discover/src/rom.rs. A catalog that
# disagrees with the discovery crate about byte order would silently key rows
# to the wrong normalized digest.
MAGIC_Z64 = 0x8037_1240
MAGIC_N64 = 0x4012_3780
MAGIC_V64 = 0x3780_4012

# IPL3 occupies ROM [0x40, 0x1000); the boot copy is the following 0x100000
# bytes. Both are hardware constants (crates/fn64-discover/src/banks.rs).
IPL3_ROM_START = 0x40
IPL3_ROM_END = 0x1000
BOOT_COPY_ROM_START = 0x1000
BOOT_COPY_SIZE = 0x0010_0000

# SHA-256 of ROM [0x40, 0x1000) -> CIC family. The first three are the digests
# crates/fn64-discover/src/banks.rs already recognizes; the last two were
# measured across this corpus and cross-checked against the IPL3 MD5/CRC32
# clusters published by Dragorn421/n64checksum (CC0-1.0).
IPL3_GROUPS = {
    "61e88238552c356c23d19409fe5570ee6910419586bc6fc740f638f761adc46e": "cic_6102_7101",
    "bf3620d30817007091ebe9bddd1b88c23b8a0052170b3309cde5b6b4238e45e7": "cic_6103_7103",
    "04b7bc6717a9f0eb724cf927e74ad3876c381cbb280d841736fc5e55580b756b": "cic_6105_7105",
    "36adc40148af56f0d78cd505eb6a90117d1fd6f11c6309e52ed36bc4c6ba340e": "cic_6106_7106",
    "16e062ba8f190c7a712a6bdb34620207299d9be676174cd81d764403df661ad0": "cic_7102",
}

# Primary opcodes a plausible MIPS III/IV instruction stream uses. Reserved
# encodings are excluded so that runs of data do not read as code.
CODE_PRIMARY_OPCODES = frozenset(
    {
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07,
        0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F,
        0x10, 0x11, 0x14, 0x15, 0x16, 0x17,
        0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27,
        0x28, 0x29, 0x2A, 0x2B, 0x2E, 0x2F,
        0x31, 0x35, 0x39, 0x3D,
    }
)

# A code run must be at least this many consecutive plausible instructions
# before its contents are admitted to the Tier 3 hazard census.
MIN_CODE_RUN_WORDS = 32

OPCODE_JAL = 0x03
OPCODE_LUI = 0x0F
WORD_JR_RA = 0x03E0_0008

# `addiu sp, sp, -N` -- the classic non-leaf prologue.
OPCODE_ADDIU = 0x09
REGISTER_SP = 29

RSP_UCODE_PATTERN = re.compile(rb"RSP (?:Gfx|SW|S2D|Audio) ucode ?[A-Za-z0-9._]{0,16}")
COMPRESSION_MARKERS = {
    "Yay0": b"Yay0",
    "MIO0": b"MIO0",
    "Yaz0": b"Yaz0",
    "gzip": b"\x1f\x8b\x08",
}
# A marker appearing once or twice is likely coincidence in 8-64 MiB of data;
# a real compressed-asset pipeline stamps its magic many times over.
COMPRESSION_MARKER_FLOOR = 3

MAX_ROM_BYTES = 64 * 1024 * 1024


class CatalogError(Exception):
    """A loud, actionable failure. Never a silent skip."""


def canonical_sorted(value: Any) -> bytes:
    try:
        return json.dumps(
            value,
            sort_keys=True,
            separators=(",", ":"),
            ensure_ascii=False,
            allow_nan=False,
        ).encode("utf-8")
    except (TypeError, ValueError) as error:
        raise CatalogError("value cannot be encoded as canonical JSON") from error


def encoded_record(record: dict[str, Any]) -> bytes:
    return canonical_sorted(record) + b"\n"


def validate_output_destination(path_text: str) -> Path:
    path = Path(path_text)
    if not path.is_absolute() or ".." in path.parts or path.name in ("", ".", ".."):
        raise CatalogError("output must be an absolute new file path without '..'")
    parent = path.parent
    try:
        if parent.resolve(strict=True) != parent:
            raise CatalogError("output parent must be canonical and contain no symlinks")
        parent_info = parent.lstat()
    except OSError as error:
        raise CatalogError("cannot inspect output parent") from error
    if not stat.S_ISDIR(parent_info.st_mode):
        raise CatalogError("output parent must be an existing directory")
    try:
        path.lstat()
    except FileNotFoundError:
        return path
    except OSError as error:
        raise CatalogError("cannot inspect output destination") from error
    raise CatalogError("refusing to overwrite existing output destination")


def publish_records(path: Path, records: Iterable[dict[str, Any]]) -> None:
    """Publish complete JSONL through a same-directory no-clobber rename."""
    validate_output_destination(str(path))
    payload = b"".join(encoded_record(record) for record in records)
    temporary = path.parent / f".{path.name}.tmp-{os.getpid()}-{secrets.token_hex(8)}"
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    descriptor = os.open(temporary, flags, 0o644)
    try:
        os.write(descriptor, payload)
        os.fsync(descriptor)
    finally:
        os.close(descriptor)
    try:
        os.link(temporary, path)
    except FileExistsError as error:
        raise CatalogError("refusing to overwrite existing output destination") from error
    finally:
        os.unlink(temporary)


def normalize_to_big_endian(data: bytes) -> tuple[bytes, str]:
    """Return canonical big-endian bytes and the source byte order."""
    if len(data) < IPL3_ROM_END:
        raise CatalogError("ROM is smaller than its header region")
    magic = struct.unpack_from(">I", data, 0)[0]
    if magic == MAGIC_Z64:
        return data, "z64"
    if len(data) % 4 != 0:
        raise CatalogError("byte-swapped ROM length is not word-aligned")
    if magic == MAGIC_N64:
        words = struct.unpack(f">{len(data) // 4}I", data)
        return struct.pack(f"<{len(words)}I", *words), "n64"
    if magic == MAGIC_V64:
        halves = struct.unpack(f">{len(data) // 2}H", data)
        return struct.pack(f"<{len(halves)}H", *halves), "v64"
    raise CatalogError(f"unknown ROM magic {magic:#010x}")


def shannon_entropy(data: bytes) -> float:
    if not data:
        return 0.0
    counts = collections.Counter(data)
    total = len(data)
    return -sum((n / total) * math.log2(n / total) for n in counts.values())


def read_header_fields(rom: bytes) -> dict[str, Any]:
    """Tier 1. Offsets mirror crates/fn64-discover/src/rom.rs RomHeader."""
    name = rom[0x20:0x34].decode("ascii", "replace").strip().strip("\x00").strip()
    cartridge_id = rom[0x3B:0x3F]
    return {
        "internal_name": name,
        "cartridge_id": cartridge_id.decode("ascii", "replace"),
        "media_format": chr(cartridge_id[0]) if 0x20 <= cartridge_id[0] < 0x7F else "",
        "cartridge_code": cartridge_id[1:3].decode("ascii", "replace"),
        "region": chr(cartridge_id[3]) if 0x20 <= cartridge_id[3] < 0x7F else "",
        "version": rom[0x3F],
        "pi_bsd_dom1_config": struct.unpack_from(">I", rom, 0x00)[0],
        "clock_rate": struct.unpack_from(">I", rom, 0x04)[0],
        "entry_point": struct.unpack_from(">I", rom, 0x08)[0],
        "libultra_version": struct.unpack_from(">I", rom, 0x0C)[0],
        "crc1": struct.unpack_from(">I", rom, 0x10)[0],
        "crc2": struct.unpack_from(">I", rom, 0x14)[0],
    }


def code_run_spans(words: list[int]) -> list[tuple[int, int]]:
    """Half-open [start, end) word spans of >= MIN_CODE_RUN_WORDS code words."""
    spans: list[tuple[int, int]] = []
    run_start = 0
    run = 0
    for index, word in enumerate(words):
        if (word >> 26) in CODE_PRIMARY_OPCODES:
            if run == 0:
                run_start = index
            run += 1
            continue
        if run >= MIN_CODE_RUN_WORDS:
            spans.append((run_start, run_start + run))
        run = 0
    if run >= MIN_CODE_RUN_WORDS:
        spans.append((run_start, run_start + run))
    return spans


def measure_boot_bank(rom: bytes) -> dict[str, Any]:
    """Tiers 2 and 3, from a single decode of the boot copy."""
    boot = rom[BOOT_COPY_ROM_START : BOOT_COPY_ROM_START + BOOT_COPY_SIZE]
    if len(boot) < 4:
        raise CatalogError("ROM is too small to contain a boot copy")
    words = list(struct.unpack(f">{len(boot) // 4}I", boot[: len(boot) // 4 * 4]))

    jal_targets: set[int] = set()
    jr_ra_count = 0
    stack_prologue_count = 0
    code_words = 0
    for word in words:
        opcode = word >> 26
        if opcode == OPCODE_JAL:
            jal_targets.add((word & 0x03FF_FFFF) << 2)
        elif word == WORD_JR_RA:
            jr_ra_count += 1
        elif (
            opcode == OPCODE_ADDIU
            and (word >> 21) & 0x1F == REGISTER_SP
            and (word >> 16) & 0x1F == REGISTER_SP
            and word & 0x8000
        ):
            stack_prologue_count += 1
        if opcode in CODE_PRIMARY_OPCODES:
            code_words += 1

    hazards = collections.Counter()
    code_run_words = 0
    for start, end in code_run_spans(words):
        code_run_words += end - start
        for index in range(start, end):
            word = words[index]
            opcode = word >> 26
            if opcode == 0x2F:
                hazards["cache_ops"] += 1
            elif opcode in (0x35, 0x3D):
                hazards["ldc1_sdc1"] += 1
            elif opcode in (0x22, 0x26, 0x2A, 0x2E):
                hazards["unaligned_mem"] += 1
            elif opcode in (0x14, 0x15, 0x16, 0x17):
                hazards["branch_likely"] += 1
            elif opcode == 0x10:
                hazards["cop0_ops"] += 1
                if (word >> 21) & 0x1F == 0:
                    hazards["mfc0"] += 1

    distinct_jal_targets = len(jal_targets)
    return {
        "boot_entropy": round(shannon_entropy(boot[:65536]), 4),
        "valid_opcode_share": round(code_words / len(words), 4) if words else 0.0,
        "distinct_jal_targets": distinct_jal_targets,
        "jr_ra_count": jr_ra_count,
        "stack_prologue_count": stack_prologue_count,
        # A boot bank that is purely a loader stub has many outbound call
        # targets and almost no resident function bodies. Guard the divisor:
        # zero returns means every target is outbound, which is the extreme of
        # the stub case, not an error.
        "loader_stub_ratio": (
            round(distinct_jal_targets / jr_ra_count, 4)
            if jr_ra_count
            else float(distinct_jal_targets)
        ),
        "code_run_bytes": code_run_words * 4,
        "code_run_share": round(code_run_words / len(words), 4) if words else 0.0,
        "unaligned_mem": hazards["unaligned_mem"],
        "cache_ops": hazards["cache_ops"],
        "branch_likely": hazards["branch_likely"],
        "ldc1_sdc1": hazards["ldc1_sdc1"],
        "cop0_ops": hazards["cop0_ops"],
        "mfc0": hazards["mfc0"],
    }


def catalog_rom(path: Path) -> dict[str, Any]:
    info = path.lstat()
    if not stat.S_ISREG(info.st_mode):
        raise CatalogError(f"{path.name} is not a regular file")
    if info.st_size > MAX_ROM_BYTES:
        raise CatalogError(f"{path.name} exceeds the {MAX_ROM_BYTES}-byte bound")
    raw = path.read_bytes()
    rom, byte_order = normalize_to_big_endian(raw)

    ipl3 = rom[IPL3_ROM_START:IPL3_ROM_END]
    ipl3_sha256 = hashlib.sha256(ipl3).hexdigest()

    ucode = sorted(
        {match.decode("ascii", "replace").strip() for match in RSP_UCODE_PATTERN.findall(rom)}
    )
    compression = {
        name: rom.count(marker)
        for name, marker in COMPRESSION_MARKERS.items()
        if rom.count(marker) >= COMPRESSION_MARKER_FLOOR
    }

    record: dict[str, Any] = {
        "schema": CATALOG_SCHEMA,
        "normalized_rom_sha256": hashlib.sha256(rom).hexdigest(),
        "sha1": hashlib.sha1(rom).hexdigest(),
        "md5": hashlib.md5(rom).hexdigest(),
        # CRC32 of the on-disk bytes is the join key for external DAT catalogs.
        "file_crc32": format(zlib.crc32(raw) & 0xFFFF_FFFF, "08x"),
        "size_bytes": len(rom),
        "byte_order": byte_order,
        "ipl3_sha256": ipl3_sha256,
        "ipl3_group": IPL3_GROUPS.get(ipl3_sha256, "unrecognized"),
        "rsp_ucode_strings": ucode,
        "compression_markers": compression,
    }
    record.update(read_header_fields(rom))
    record.update(measure_boot_bank(rom))
    return record


def stable_id(path: Path) -> str:
    """A path-free, lowercase, filesystem-independent identifier."""
    slug = re.sub(r"[^a-z0-9]+", "-", path.stem.lower()).strip("-")
    return slug or "rom"


def discover_roms(directory: Path) -> list[Path]:
    if not directory.is_dir():
        raise CatalogError(f"{directory} is not a directory")
    roms = sorted(
        entry
        for entry in directory.iterdir()
        if entry.is_file() and entry.suffix.lower() in (".z64", ".n64", ".v64")
    )
    if not roms:
        raise CatalogError(f"no ROMs found in {directory}")
    return roms


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument(
        "--rom-dir",
        help="ROM directory; defaults to $FN64_ROM_CORPUS_DIR. No relative fallback.",
    )
    result.add_argument("--output", help="absolute JSONL path; omit to stream to stdout")
    return result


def main() -> int:
    try:
        args = parser().parse_args()
        rom_dir_text = args.rom_dir or os.environ.get("FN64_ROM_CORPUS_DIR")
        if not rom_dir_text:
            raise CatalogError(
                "set --rom-dir or FN64_ROM_CORPUS_DIR; there is no default ROM location"
            )
        output_path = validate_output_destination(args.output) if args.output else None

        records = []
        for rom_path in discover_roms(Path(rom_dir_text)):
            record = catalog_rom(rom_path)
            record["stable_id"] = stable_id(rom_path)
            records.append(record)

        ids = [record["stable_id"] for record in records]
        if len(set(ids)) != len(ids):
            duplicates = sorted({name for name in ids if ids.count(name) > 1})
            raise CatalogError(f"stable_id collision: {', '.join(duplicates)}")

        if output_path is None:
            for record in records:
                sys.stdout.buffer.write(encoded_record(record))
        else:
            publish_records(output_path, records)
        return 0
    except CatalogError as error:
        print(f"rom-catalog: {error}", file=sys.stderr)
        return 1
    except OSError as error:
        print(f"rom-catalog: operating-system error ({error.errno})", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
