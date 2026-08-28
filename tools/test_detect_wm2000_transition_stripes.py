from __future__ import annotations

import struct
import pathlib
import sys
import unittest
import zlib

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from detect_wm2000_transition_stripes import decode_rgba8_png, transition_stripe_score


def png(width: int, height: int, rgba: bytes) -> bytes:
    def chunk(kind: bytes, payload: bytes) -> bytes:
        return (
            struct.pack(">I", len(payload))
            + kind
            + payload
            + struct.pack(">I", zlib.crc32(kind + payload) & 0xFFFF_FFFF)
        )

    filtered = b"".join(
        b"\0" + rgba[y * width * 4 : (y + 1) * width * 4] for y in range(height)
    )
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(filtered))
        + chunk(b"IEND", b"")
    )


class TransitionStripeDetectorTests(unittest.TestCase):
    def test_rgba8_png_round_trip(self) -> None:
        rgba = bytes(range(4 * 7 * 5))
        self.assertEqual(decode_rgba8_png(png(7, 5, rgba)), (7, 5, rgba))

    def test_repeated_captured_slope_is_distinct_from_flat_frame(self) -> None:
        width, height = 479, 237
        flat = bytearray([96, 96, 96, 255] * width * height)
        striped = bytearray(flat)
        for y in range(height):
            for x in range(width):
                if (2 * y - x) % 96 == 93:
                    offset = (y * width + x) * 4
                    striped[offset : offset + 4] = bytes((255, 255, 255, 255))
        self.assertEqual(transition_stripe_score(width, height, flat), 0)
        self.assertGreaterEqual(transition_stripe_score(width, height, striped), 400)


if __name__ == "__main__":
    unittest.main()
