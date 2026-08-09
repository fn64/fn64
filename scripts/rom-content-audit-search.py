#!/usr/bin/env python3
"""Search a built Mach-O binary for verbatim byte runs from the user's ROM.

This settles falsification item #1 of docs/plans/rom-content-in-shipped-artifact.md:
"the no-embedded-ROM-data claim rests on reading the generators, not on
inspecting a binary."

DESIGN NOTES (each earned from a way this search can silently return a false
zero -- perf-method rules 6a/19/20):

1. ENDIANNESS. An N64 z64 ROM is big-endian. The generators emit ROM words as
   Rust `u32` literals into a `&[u32]`. On arm64 (little-endian) each such word
   is stored BYTE-REVERSED relative to the ROM. A search for the raw ROM bytes
   alone therefore CANNOT find the arrays that are known to be there. We search
   four orderings:
     z64   -- raw ROM bytes (big-endian, as on disc)
     swap4 -- each 4-byte word byte-reversed  (== the little-endian u32 form,
              and also the `.n64` little-endian ROM ordering)
     swap2 -- each 2-byte halfword swapped    (== the `.v64` byteswapped ordering)
     swap2of4 -- 16-bit halfwords swapped within each 32-bit word
   Any hit in ANY ordering is a hit.

2. POSITIVE CONTROL. Before reporting any absence, the search MUST demonstrate
   it can find content that is known present. The un-gated (verify-on) lane
   emits EXPECTED_WORDS arrays of literal ROM words from the boot copy at ROM
   0x1000. If the search does not light up there, the search is broken and every
   zero it reports is meaningless. `--require-control` makes that a hard failure.

3. SAMPLING BREADTH. Samples are taken across the WHOLE ROM on a fixed stride,
   not from the start. Three separate errors in this project came from sampling
   one region and generalizing. Low-entropy runs (long zero/0xFF fills, few
   distinct bytes) are skipped because they match by coincidence rather than by
   provenance and would manufacture false positives.
"""

import argparse
import bisect
import collections
import json
import subprocess
import sys


# ---------------------------------------------------------------- orderings

def swap4(b: bytes) -> bytes:
    n = len(b) & ~3
    out = bytearray(b[:n])
    out[0::4], out[1::4], out[2::4], out[3::4] = out[3::4], out[2::4], out[1::4], out[0::4]
    return bytes(out)


def swap2(b: bytes) -> bytes:
    n = len(b) & ~1
    out = bytearray(b[:n])
    out[0::2], out[1::2] = out[1::2], out[0::2]
    return bytes(out)


def swap2of4(b: bytes) -> bytes:
    n = len(b) & ~3
    out = bytearray(b[:n])
    out[0::4], out[1::4], out[2::4], out[3::4] = out[2::4], out[3::4], out[0::4], out[1::4]
    return bytes(out)


ORDERINGS = {
    "z64": lambda b: b,
    "swap4": swap4,     # little-endian u32 -- the form Rust arrays take on arm64
    "swap2": swap2,     # .v64
    "swap2of4": swap2of4,
}


# ---------------------------------------------------------------- Mach-O map

def macho_sections(path):
    """Return [(fileoff, size, 'SEG,SECT')] from otool -l, sorted by fileoff."""
    out = subprocess.run(["otool", "-l", path], capture_output=True, text=True).stdout
    # NOTE: within an otool `Section` block the field order is
    #   sectname / segname / addr / size / offset / ...
    # i.e. `size` precedes `offset`. An earlier version of this parser only
    # emitted a section when it saw `size` AFTER `offset` and therefore mapped
    # ZERO sections on every binary -- which would have rendered every
    # "which section is this hit in?" answer "(outside any section)" while the
    # hit counts stayed correct. Caught by the self-test's section count, not by
    # the hit count. Do not reorder these branches.
    secs, cur, in_sect = [], {}, False
    for line in out.splitlines():
        s = line.strip()
        if s.startswith("Section"):
            cur, in_sect = {}, True
        elif not in_sect:
            continue
        elif s.startswith("sectname "):
            cur["sect"] = s.split(None, 1)[1]
        elif s.startswith("segname "):
            cur["seg"] = s.split(None, 1)[1]
        elif s.startswith("size "):
            v = s.split()[1]
            cur["size"] = int(v, 16) if v.startswith("0x") else int(v)
        elif s.startswith("offset "):
            cur["off"] = int(s.split()[1], 0)
            if "sect" in cur and "size" in cur:
                secs.append((cur["off"], cur["size"],
                             f"{cur.get('seg','?')},{cur['sect']}"))
            cur, in_sect = {}, False
    secs.sort()
    return secs


def locate(secs, starts, off):
    i = bisect.bisect_right(starts, off) - 1
    if i >= 0:
        o, sz, name = secs[i]
        if o <= off < o + sz:
            return name
    return "(outside any section)"


# ---------------------------------------------------------------- sampling

def interesting(run: bytes, min_distinct: int) -> bool:
    """Reject low-entropy runs: they match by coincidence, not provenance."""
    if len(set(run)) < min_distinct:
        return False
    # reject runs dominated by one byte value (padding, fills)
    top = collections.Counter(run).most_common(1)[0][1]
    return top <= len(run) * 0.5


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--rom", required=True)
    ap.add_argument("--binary", required=True, action="append",
                    help="repeatable; each is labelled by its path")
    ap.add_argument("--run-len", type=int, default=64)
    ap.add_argument("--stride", type=lambda s: int(s, 0), default=0x8000,
                    help="ROM sampling stride")
    ap.add_argument("--min-distinct", type=int, default=24)
    ap.add_argument("--control-offset", type=lambda s: int(s, 0), default=0x1000,
                    help="ROM offset known to be embedded in the un-gated lane")
    ap.add_argument("--require-control", metavar="BINARY",
                    help="fail unless a control hit is found in this binary")
    ap.add_argument("--no-synthetic-control", action="store_true",
                    help="skip the binary-independent self-test (not advised)")
    ap.add_argument("--require-clean", metavar="BINARY", action="append",
                    help="repeatable; fail unless this binary has ZERO ROM-word hits")
    ap.add_argument("--json-out")
    args = ap.parse_args()

    rom = open(args.rom, "rb").read()
    print(f"ROM {args.rom}: {len(rom)} bytes ({len(rom)/2**20:.1f} MiB)")

    # --- rule 6a, binary-INDEPENDENT: prove the matcher works before using it.
    #
    # `--require-control` uses the un-gated binary as the positive control, but
    # that only works while some binary still embeds ROM words. Once BOTH lanes
    # are clean -- which is the whole point of the geometry substitution -- that
    # control has nothing to find and would either fail or be quietly dropped,
    # leaving a zero that proves nothing.
    #
    # This control depends on no binary at all: synthesize a buffer containing a
    # known ROM run in each ordering and confirm the matcher finds each one, and
    # confirm it does NOT find a run that was never planted. A search that fails
    # here is broken regardless of what the binaries contain.
    if not args.no_synthetic_control:
        probe_off = args.control_offset
        probe = rom[probe_off:probe_off + args.run_len]
        planted = {}
        haystack = bytearray()
        for oname, fn in ORDERINGS.items():
            haystack += bytes(64)                 # padding so offsets differ
            planted[oname] = len(haystack)
            haystack += fn(probe)
        absent = bytes((b ^ 0xA5) for b in ORDERINGS["z64"](probe))
        synth_ok = True
        for oname, fn in ORDERINGS.items():
            needle = fn(probe)
            at = bytes(haystack).find(needle)
            if at != planted[oname]:
                print(f"  SYNTHETIC CONTROL FAIL: ordering {oname} planted at "
                      f"{planted[oname]:#x} but found at {at:#x}", file=sys.stderr)
                synth_ok = False
        if bytes(haystack).find(absent) != -1:
            print("  SYNTHETIC CONTROL FAIL: matched a run that was never planted "
                  "-- the matcher reports false positives", file=sys.stderr)
            synth_ok = False
        n_ord = len(ORDERINGS)
        n_found = sum(
            1 for oname, fn in ORDERINGS.items()
            if bytes(haystack).find(fn(probe)) == planted[oname]
        )
        print(f"synthetic control: {n_found}/{n_ord} orderings planted and found at "
              f"ROM {probe_off:#x} -- {'PASS' if synth_ok else 'FAIL'}")
        if not synth_ok:
            print("\nThe matcher itself is broken. Every zero below is meaningless.",
                  file=sys.stderr)
            sys.exit(2)

    # --- build the sample set: fixed stride across the WHOLE ROM
    samples = []          # (rom_off, run_bytes)
    skipped_low_entropy = 0
    for off in range(0, len(rom) - args.run_len, args.stride):
        run = rom[off:off + args.run_len]
        if interesting(run, args.min_distinct):
            samples.append((off, run))
        else:
            skipped_low_entropy += 1
    # plus named regions of interest, forced in regardless of stride
    named = {
        0x0000: "ROM header (magic/clock/PC/title)",
        0x0040: "IPL3 / CIC boot code (start)",
        0x0400: "IPL3 / CIC boot code (mid)",
        0x0800: "IPL3 / CIC boot code (late)",
        0x1000: "boot copy start (first DMA'd word)",
        0x1004: "boot copy +4",
        0x8000: "boot copy interior",
        0x40000: "boot copy interior (256K)",
        0xF0000: "boot copy near end (960K)",
        0x100000: "just past the 1 MiB boot copy",
        0x200000: "2 MiB -- overlay/asset region",
        0x800000: "8 MiB -- deep data region",
        0x1000000: "16 MiB -- deep data region",
        0x1A00000: "26 MiB -- last non-padding megabyte",
    }
    have = {o for o, _ in samples}
    for off, label in named.items():
        if off + args.run_len <= len(rom) and off not in have:
            run = rom[off:off + args.run_len]
            samples.append((off, run))
    samples.sort()
    print(f"samples: {len(samples)} runs of {args.run_len} B "
          f"(stride {args.stride:#x}, {skipped_low_entropy} low-entropy runs skipped)")
    print(f"coverage: ROM offsets {samples[0][0]:#x} .. {samples[-1][0]:#x}")

    results = {}
    for binpath in args.binary:
        blob = open(binpath, "rb").read()
        secs = macho_sections(binpath)
        starts = [s[0] for s in secs]
        print(f"\n{'='*78}\nBINARY {binpath}")
        print(f"  {len(blob)} bytes ({len(blob)/2**20:.2f} MiB), "
              f"{len(secs)} Mach-O sections mapped")
        if not secs:
            print("  !! otool -l returned no sections -- TOOLING IS BROKEN, "
                  "do not trust any zero below")

        hits = []
        for rom_off, run in samples:
            for oname, fn in ORDERINGS.items():
                needle = fn(run)
                if len(needle) < args.run_len:
                    continue
                pos, found = 0, []
                while True:
                    i = blob.find(needle, pos)
                    if i < 0:
                        break
                    found.append(i)
                    pos = i + 1
                    if len(found) >= 8:
                        break
                for i in found:
                    hits.append({
                        "rom_off": rom_off,
                        "ordering": oname,
                        "bin_off": i,
                        "section": locate(secs, starts, i),
                        "named": named.get(rom_off),
                    })

        results[binpath] = {"size": len(blob), "hits": hits}
        by_ord = collections.Counter(h["ordering"] for h in hits)
        by_sect = collections.Counter(h["section"] for h in hits)
        distinct_rom = sorted({h["rom_off"] for h in hits})
        print(f"  HITS: {len(hits)} total, from {len(distinct_rom)} distinct ROM offsets")
        print(f"  by ordering: {dict(by_ord)}")
        print(f"  by section:  {dict(by_sect)}")
        if distinct_rom:
            lo, hi = distinct_rom[0], distinct_rom[-1]
            print(f"  ROM offset span of hits: {lo:#x} .. {hi:#x}")
            print("  first 40 hits:")
            for h in hits[:40]:
                tag = f"  <- {h['named']}" if h["named"] else ""
                print(f"    rom {h['rom_off']:#09x} [{h['ordering']:>8}] -> "
                      f"bin {h['bin_off']:#010x} in {h['section']}{tag}")
            if len(hits) > 40:
                print(f"    ... {len(hits)-40} more")

    # --- rule 6a: prove the search CAN find something before trusting a zero
    ok = True
    if args.require_control:
        ctrl = results.get(args.require_control)
        if ctrl is None:
            print(f"\nPOSITIVE CONTROL: {args.require_control} was not searched", file=sys.stderr)
            ok = False
        else:
            n = len(ctrl["hits"])
            print(f"\n{'='*78}\nPOSITIVE CONTROL ({args.require_control}): {n} hits")
            if n == 0:
                print("  FAIL -- the search found NOTHING in a binary known to embed ROM\n"
                      "  words. The search is broken; every zero it reports is meaningless.",
                      file=sys.stderr)
                ok = False
            else:
                print("  PASS -- the search demonstrably finds embedded ROM content.\n"
                      "  Absences reported for other binaries are therefore meaningful.")

    # --- the acceptance assertion: named binaries must carry NO ROM words.
    # Only meaningful because the synthetic control above already proved the
    # matcher finds planted content in every ordering.
    for binpath in (args.require_clean or []):
        res = results.get(binpath)
        print(f"\n{'='*78}\nCLEAN REQUIREMENT ({binpath})")
        if res is None:
            print("  FAIL -- was not searched", file=sys.stderr)
            ok = False
            continue
        n = len(res["hits"])
        if n:
            offs = sorted({h["rom_off"] for h in res["hits"]})
            print(f"  FAIL -- {n} hits from {len(offs)} distinct ROM offsets; "
                  f"first: {', '.join(hex(o) for o in offs[:8])}", file=sys.stderr)
            ok = False
        else:
            print("  PASS -- zero verbatim ROM runs found, in any of the "
                  f"{len(ORDERINGS)} orderings.")

    if args.json_out:
        json.dump(results, open(args.json_out, "w"), indent=1)
        print(f"\njson -> {args.json_out}")

    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
