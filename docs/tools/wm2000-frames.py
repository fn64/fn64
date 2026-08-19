#!/usr/bin/env python3
"""Inspect a WM2000 frame-dump directory: which frames carry content, and is
the screen advancing or looping?

Written because this was hand-rolled at least four times in one session, twice
with a bug. Two questions come up constantly and both are cheap to answer from
the PNGs alone:

  1. Which frame should I look at? -- rank by distinct byte values. A blank or
     uniform field has 2; real composed content runs into the hundreds or
     thousands. Picking a frame by eye from thousands of dumps is guesswork.

  2. Is the screen advancing, or idling? -- hash each frame. A screen that
     holds one hash is stalled; one cycling a short repeating set is idling on
     a condition at full compose rate (that shape is a plateau, not a hang, and
     the distinction decided a whole investigation); all-distinct hashes mean
     live animation.

Frame hash alone is NOT a reliable advancement signal -- a screen can advance
while the hash holds. Cross-check with the gfx-task rate (wm2000-gfx-rate.py)
before concluding a screen did not change.

Reads only the PNGs the harness writes; no ROM, no emulator, no dependencies.

    wm2000-frames.py <dump-dir> [--top N] [--tail N]
"""
import argparse
import hashlib
import os
import re
import struct
import sys
import zlib

FRAME_RE = re.compile(r"fn64-fb-(\d+)\.png$")


def raw_pixels(path):
    """Return (width, height, decompressed IDAT bytes) for a PNG."""
    data = open(path, "rb").read()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError(f"{path}: not a PNG")
    width, height = struct.unpack(">II", data[16:24])
    idat = b""
    i = 8
    while i < len(data):
        length = struct.unpack(">I", data[i : i + 4])[0]
        kind = data[i + 4 : i + 8]
        if kind == b"IDAT":
            idat += data[i + 8 : i + 8 + length]
        i += 12 + length
    return width, height, zlib.decompress(idat)


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("dump_dir")
    ap.add_argument("--top", type=int, default=5,
                    help="how many richest frames to list (default 5)")
    ap.add_argument("--tail", type=int, default=30,
                    help="how many trailing frames to check for looping (default 30)")
    args = ap.parse_args()

    frames = []
    for name in os.listdir(args.dump_dir):
        m = FRAME_RE.match(name)
        if m:
            frames.append((int(m.group(1)), os.path.join(args.dump_dir, name)))
    if not frames:
        print(f"no fn64-fb-<swap>.png frames in {args.dump_dir}", file=sys.stderr)
        return 1
    frames.sort()

    print(f"frames: {len(frames)}  swaps {frames[0][0]}..{frames[-1][0]}")

    scored = []
    for swap, path in frames:
        try:
            w, h, raw = raw_pixels(path)
        except Exception as exc:  # a truncated dump from a killed run
            print(f"  swap {swap}: unreadable ({exc})")
            continue
        scored.append((len(set(raw)), swap, w, h))
    if not scored:
        return 1

    # Geometry is reported, not assumed: a 320-wide dump of this ROM is sheared
    # (docs/RT64-WM2000-HARNESS-TRAPS.md), and saying so beats silently ranking
    # corrupted frames.
    geoms = {(w, h) for _, _, w, h in scored}
    for w, h in sorted(geoms):
        note = "" if w == 480 else "  <-- NOT 480 wide: sheared, see HARNESS-TRAPS.md"
        print(f"geometry: {w}x{h}{note}")

    print(f"\nrichest {args.top} frames (distinct byte values; blank field is 2):")
    for distinct, swap, _, _ in sorted(scored, reverse=True)[: args.top]:
        print(f"  swap {swap:>6}  distinct={distinct}")

    tail = frames[-args.tail :]
    hashes = []
    for swap, path in tail:
        try:
            _, _, raw = raw_pixels(path)
        except Exception:
            continue
        hashes.append(hashlib.sha256(raw).hexdigest()[:12])
    if hashes:
        distinct = len(set(hashes))
        print(f"\nlast {len(hashes)} frames: {distinct} distinct hashes")
        if distinct == 1:
            print("  STALLED: one frame repeating -- nothing is being composed anew")
        elif distinct < len(hashes):
            print("  LOOPING: a short repeating set -- typically a screen idling on a")
            print("  condition at full compose rate, not a hang. Cross-check the")
            print("  gfx-task rate before calling it a plateau.")
        else:
            print("  ADVANCING: every frame distinct -- live animation")
    return 0


if __name__ == "__main__":
    sys.exit(main())
