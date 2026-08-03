#!/usr/bin/env python3
"""Recover developer source paths left in ROM bytes, and the engine families
they reveal.

Debug builds leak __FILE__ strings, assert paths, and printf formats into the
shipped image. Those are measured observations of bytes we already have -- no
answer key and no decompilation project involved -- and they identify shared
middleware across titles that no metadata field captures: two ROMs quoting the
same private source path were built from the same tree.

Measured over the 287-ROM corpus: 79 ROMs leak at least one path-shaped
string, 898 distinct paths total, and 36 paths appear in more than one ROM.
Those shared paths resolve into engine families -- Ubisoft (Donald Duck,
Rayman 2, Tonic Trouble; 11 shared paths including
`../../Public/Geo/GeoBdVol.h`), Acclaim/Turok (Armorines, Shadow Man, South
Park, Turok Rage Wars, Turok 2, Turok 3; 10 shared), and Rare (Jet Force
Gemini, Mickey's Speedway).

That grouping is directly useful for discovery work. A fix aimed at one engine
can be predicted to cover its whole family before it is written, and a family
member that already passes a gate is a ready-made control for one that does
not -- Tonic Trouble passes `gate_rom_rebuild` while Donald Duck and Rayman 2
fail it identically, which is what isolates their shared defect.

Matching anchors on the file extension and then walks backwards over printable
path bytes. An earlier formulation put a lazy prefix before a nested group and
backtracked catastrophically: it ran twelve hours over this corpus without
finishing. This form is linear in ROM size and completes in seconds.
"""

import argparse
import collections
import glob
import os
import re

# Anchor on the extension; everything before it is recovered by walking back.
EXTENSION = re.compile(rb"\.(?:c|h|cpp|s|asm)\b")

# Bytes admissible in a source path. Restricting to this set is what rejects
# float and texture data that happens to precede an extension-shaped run.
PATH_BYTES = frozenset(
    bytes(range(0x30, 0x3A))
    + b"ABCDEFGHIJKLMNOPQRSTUVWXYZ"
    + b"abcdefghijklmnopqrstuvwxyz"
    + b"_-./\\:"
)

# A path with no separator is a bare filename, which collides with ordinary
# strings; requiring one, plus a minimum length, keeps the yield honest.
MIN_PATH_BYTES = 8
MAX_PATH_BYTES = 80


def source_paths(rom_bytes):
    """Every path-shaped source reference in one ROM image."""
    found = set()
    for match in EXTENSION.finditer(rom_bytes):
        end = match.end()
        start = match.start()
        while (
            start > 0
            and rom_bytes[start - 1] in PATH_BYTES
            and end - start < MAX_PATH_BYTES
        ):
            start -= 1
        candidate = rom_bytes[start:end]
        if len(candidate) < MIN_PATH_BYTES:
            continue
        if b"/" not in candidate and b"\\" not in candidate:
            continue
        found.add(candidate)
    return found


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--rom-dir", default="/Users/jer/Code/roms/n64")
    parser.add_argument("--top", type=int, default=15)
    arguments = parser.parse_args()

    per_rom = {}
    for path in sorted(glob.glob(os.path.join(arguments.rom_dir, "*.z64"))):
        with open(path, "rb") as handle:
            rom_bytes = handle.read()
        if rom_bytes[:4] != b"\x80\x37\x12\x40":
            continue
        paths = source_paths(rom_bytes)
        if paths:
            per_rom[os.path.basename(path)] = paths

    total = sum(len(paths) for paths in per_rom.values())
    print(f"ROMs leaking source paths: {len(per_rom)}  distinct paths: {total}")
    print()
    ranked = sorted(per_rom.items(), key=lambda item: -len(item[1]))
    for rom, paths in ranked[: arguments.top]:
        sample = sorted(paths)[0].decode("ascii", "replace")
        print(f"{len(paths):>5}  {rom[:44]:46} {sample[:50]}")

    # A path in more than one ROM is shared-tree evidence: an engine family.
    by_path = collections.defaultdict(list)
    for rom, paths in per_rom.items():
        for path in paths:
            by_path[path].append(rom)
    families = collections.Counter()
    for roms in by_path.values():
        if len(roms) > 1:
            families[tuple(sorted(roms))] += 1

    print()
    print(f"paths shared across ROMs: {sum(families.values())}")
    for roms, count in families.most_common(8):
        names = ", ".join(rom.split(" (")[0][:26] for rom in roms)
        print(f"  {count:>4} shared  {names}")


main()
