#!/usr/bin/env python3
"""Detect WM2000's repeated 96x48 transition-quad diagonal artifact."""

from __future__ import annotations

import argparse
import json
import pathlib
import re
import struct
import zlib


PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"
FRAME_NAME = re.compile(r"^frame-(\d+)-[0-9a-f]{16}\.png$")
DEFAULT_RANGES = ((115, 144), (211, 240), (307, 336), (415, 442))


def _paeth(left: int, above: int, upper_left: int) -> int:
    estimate = left + above - upper_left
    left_distance = abs(estimate - left)
    above_distance = abs(estimate - above)
    upper_left_distance = abs(estimate - upper_left)
    if left_distance <= above_distance and left_distance <= upper_left_distance:
        return left
    if above_distance <= upper_left_distance:
        return above
    return upper_left


def decode_rgba8_png(encoded: bytes) -> tuple[int, int, bytes]:
    """Decode the non-interlaced RGBA8 PNGs emitted by fn64's frame dump."""
    if not encoded.startswith(PNG_SIGNATURE):
        raise ValueError("not a PNG")
    cursor = len(PNG_SIGNATURE)
    width = height = None
    compressed = bytearray()
    while cursor < len(encoded):
        if cursor + 12 > len(encoded):
            raise ValueError("truncated PNG chunk")
        length = struct.unpack_from(">I", encoded, cursor)[0]
        kind = encoded[cursor + 4 : cursor + 8]
        payload_start = cursor + 8
        payload_end = payload_start + length
        if payload_end + 4 > len(encoded):
            raise ValueError("truncated PNG payload")
        payload = encoded[payload_start:payload_end]
        if kind == b"IHDR":
            if length != 13:
                raise ValueError("malformed IHDR")
            width, height, depth, color, compression, filtering, interlace = (
                struct.unpack(">IIBBBBB", payload)
            )
            if (depth, color, compression, filtering, interlace) != (8, 6, 0, 0, 0):
                raise ValueError("frame dump must be non-interlaced RGBA8")
        elif kind == b"IDAT":
            compressed.extend(payload)
        elif kind == b"IEND":
            break
        cursor = payload_end + 4
    if width is None or height is None or not compressed:
        raise ValueError("PNG is missing IHDR or IDAT")

    row_bytes = width * 4
    filtered = zlib.decompress(compressed)
    if len(filtered) != height * (row_bytes + 1):
        raise ValueError("unexpected decompressed PNG length")
    rgba = bytearray(width * height * 4)
    prior = bytearray(row_bytes)
    source = 0
    for y in range(height):
        filter_kind = filtered[source]
        source += 1
        raw = filtered[source : source + row_bytes]
        source += row_bytes
        row = bytearray(row_bytes)
        for x, byte in enumerate(raw):
            left = row[x - 4] if x >= 4 else 0
            above = prior[x]
            upper_left = prior[x - 4] if x >= 4 else 0
            if filter_kind == 0:
                prediction = 0
            elif filter_kind == 1:
                prediction = left
            elif filter_kind == 2:
                prediction = above
            elif filter_kind == 3:
                prediction = (left + above) // 2
            elif filter_kind == 4:
                prediction = _paeth(left, above, upper_left)
            else:
                raise ValueError(f"unsupported PNG filter {filter_kind}")
            row[x] = (byte + prediction) & 0xFF
        start = y * row_bytes
        rgba[start : start + row_bytes] = row
        prior = row
    return width, height, bytes(rgba)


def transition_stripe_score(width: int, height: int, rgba: bytes) -> int:
    """Return the strongest repeated bright ridge at the captured quad slope."""
    if width < 3 or height < 5 or len(rgba) != width * height * 4:
        raise ValueError("malformed RGBA frame")

    def luma(x: int, y: int) -> int:
        offset = (y * width + x) * 4
        red, green, blue = rgba[offset : offset + 3]
        return (77 * red + 150 * green + 29 * blue) >> 8

    # Each bad shared edge advances two X pixels per Y pixel and repeats for
    # every 96x48 transition quad. A true stripe is a bright center whose two
    # perpendicular neighbours agree, accumulated by the exact repeated-edge
    # phase (2*y-x) mod 96. Natural logo edges do not concentrate in one phase.
    phases = [0] * 96
    for y in range(2, height - 2):
        for x in range(1, width - 1):
            center = luma(x, y)
            side_a = luma(x + 1, y - 2)
            side_b = luma(x - 1, y + 2)
            if abs(side_a - side_b) <= 16 and center - max(side_a, side_b) >= 32:
                phases[(2 * y - x) % 96] += 1
    return max(phases)


def frame_index(path: pathlib.Path) -> int:
    match = FRAME_NAME.fullmatch(path.name)
    if match is None:
        raise ValueError(f"noncanonical frame-dump name: {path.name}")
    return int(match.group(1))


def scan_directory(
    directory: pathlib.Path,
    ranges: tuple[tuple[int, int], ...] = DEFAULT_RANGES,
) -> list[dict[str, object]]:
    rows = []
    for path in sorted(directory.glob("frame-*.png")):
        index = frame_index(path)
        if not any(start <= index <= end for start, end in ranges):
            continue
        width, height, rgba = decode_rgba8_png(path.read_bytes())
        rows.append(
            {
                "frame": index,
                "file": path.name,
                "score": transition_stripe_score(width, height, rgba),
            }
        )
    if not rows:
        raise ValueError("no frame dumps matched the requested ranges")
    return rows


def _parse_range(value: str) -> tuple[int, int]:
    try:
        start_text, end_text = value.split(":", 1)
        start, end = int(start_text), int(end_text)
    except ValueError as error:
        raise argparse.ArgumentTypeError("range must be START:END") from error
    if start < 0 or end < start:
        raise argparse.ArgumentTypeError("range must be nonnegative and ordered")
    return start, end


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("frame_dump", type=pathlib.Path)
    parser.add_argument("--range", dest="ranges", action="append", type=_parse_range)
    parser.add_argument("--fail-at", type=int, default=100)
    args = parser.parse_args()
    if args.fail_at <= 0:
        parser.error("--fail-at must be positive")
    try:
        rows = scan_directory(args.frame_dump, tuple(args.ranges or DEFAULT_RANGES))
    except (OSError, ValueError, zlib.error) as error:
        parser.error(str(error))
    worst = max(rows, key=lambda row: int(row["score"]))
    result = {
        "schema": "fn64.wm2000-transition-stripes.v1",
        "frames": len(rows),
        "fail_at": args.fail_at,
        "worst": worst,
        "violations": [row for row in rows if int(row["score"]) >= args.fail_at],
    }
    print(json.dumps(result, indent=2, sort_keys=True))
    return 1 if result["violations"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
