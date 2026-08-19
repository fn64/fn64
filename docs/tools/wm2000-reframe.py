#!/usr/bin/env python3
"""Re-lay a WM2000 harness framebuffer dump at the VI's real 480-pixel stride.

The harness dumps `fn64-fb-<swap>.png` by reading the guest framebuffer as
320x240 RGBA5551 (packages/wm2000-boot/src/main.rs, `capture_framebuffer`).
WM2000 actually scans out 480x237 -- see docs/frames/README.md, where reading
the 480-stride buffer at a 320 stride was shown to shear every row and to have
manufactured a convincing but false "interlace striping" defect.

The dump is still a faithful record of the BYTES: 320*240 = 76800 contiguous
pixels off the same RDRAM region. Re-laying those pixels 480 to a row recovers
160 true rows -- the top two thirds of the frame, unsheared. That is enough to
read a menu screen and tell one from another, and it needs no re-run.

Rows 160..237 are simply not in the dump (the 320x240 read stops short of the
full 480x237 region); this tool never invents them.
"""
import argparse, pathlib, struct, zlib

def read_png_rgba(path):
    data = pathlib.Path(path).read_bytes()
    assert data[:8] == b"\x89PNG\r\n\x1a\n", f"{path}: not a PNG"
    pos, idat, w, h, bitd, ctype = 8, b"", 0, 0, 0, 0
    while pos < len(data):
        (ln,) = struct.unpack(">I", data[pos:pos + 4])
        typ = data[pos + 4:pos + 8]
        body = data[pos + 8:pos + 8 + ln]
        if typ == b"IHDR":
            w, h, bitd, ctype = struct.unpack(">IIBB", body[:10])
        elif typ == b"IDAT":
            idat += body
        elif typ == b"IEND":
            break
        pos += 12 + ln
    assert bitd == 8 and ctype == 6, f"{path}: expected 8-bit RGBA, got bitd={bitd} ctype={ctype}"
    raw = zlib.decompress(idat)
    bpp, stride = 4, w * 4
    out, prev = bytearray(), bytearray(stride)
    p = 0
    for _ in range(h):
        f = raw[p]; p += 1
        line = bytearray(raw[p:p + stride]); p += stride
        for i in range(stride):
            a = line[i - bpp] if i >= bpp else 0
            b = prev[i]
            c = prev[i - bpp] if i >= bpp else 0
            if f == 1:   line[i] = (line[i] + a) & 0xFF
            elif f == 2: line[i] = (line[i] + b) & 0xFF
            elif f == 3: line[i] = (line[i] + (a + b) // 2) & 0xFF
            elif f == 4:
                pp = a + b - c
                pa, pb, pc = abs(pp - a), abs(pp - b), abs(pp - c)
                pr = a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)
                line[i] = (line[i] + pr) & 0xFF
        out += line
        prev = line
    return w, h, bytes(out)

def write_png_rgba(path, w, h, px):
    raw = b"".join(b"\x00" + px[y * w * 4:(y + 1) * w * 4] for y in range(h))
    def chunk(t, d):
        c = t + d
        return struct.pack(">I", len(d)) + c + struct.pack(">I", zlib.crc32(c))
    pathlib.Path(path).write_bytes(
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b""))

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("src"); ap.add_argument("dst")
    ap.add_argument("--stride", type=int, default=480)
    a = ap.parse_args()
    w, h, px = read_png_rgba(a.src)
    total = w * h
    rows = total // a.stride
    assert rows, f"{a.src}: {total} px is fewer than one {a.stride}-px row"
    write_png_rgba(a.dst, a.stride, rows, px[:a.stride * rows * 4])
    print(f"{a.src} ({w}x{h}) -> {a.dst} ({a.stride}x{rows}) "
          f"[{total} px re-laid at stride {a.stride}]")

if __name__ == "__main__":
    main()
