#!/usr/bin/env python3
"""Losslessly recompress the reference frame PNGs, in place.

Run from the repo root after dumping frames, BEFORE committing them:

    python3 scripts/recompress-frames.py

`crates/fn64-render-reference/src/png_dump.rs` is a deliberately
dependency-free encoder: it writes "stored" (uncompressed) DEFLATE with no row
filtering, so every 480x240 dump is a flat 461 KB whatever it depicts. That is
the right trade for a debug dumper and the wrong one for committed evidence --
fourteen frames arrived at 6.5 MB and left at 1.6 MB, one near-uniform fade
frame at 1.0% of its dumped size.

Lossless by construction and by check: the RGBA pixels are decoded back from
the filtered data about to be written and asserted byte-identical to the
original before any file is touched. Idempotent, so re-running is safe.

Stdlib only -- no oxipng/pngcrush/optipng/pngquant needed.
"""
import glob, os, struct, sys, zlib


def chunks(data):
    pos = 8
    while pos < len(data):
        (ln,) = struct.unpack('>I', data[pos:pos + 4])
        typ = data[pos + 4:pos + 8]
        yield typ, data[pos + 8:pos + 8 + ln]
        pos += 12 + ln


def unfilter(raw, w, h, bpp):
    stride = w * bpp
    out = bytearray()
    prev = bytearray(stride)
    pos = 0
    for _ in range(h):
        ft = raw[pos]; pos += 1
        line = bytearray(raw[pos:pos + stride]); pos += stride
        if ft == 1:
            for i in range(bpp, stride):
                line[i] = (line[i] + line[i - bpp]) & 0xFF
        elif ft == 2:
            for i in range(stride):
                line[i] = (line[i] + prev[i]) & 0xFF
        elif ft == 3:
            for i in range(stride):
                a = line[i - bpp] if i >= bpp else 0
                line[i] = (line[i] + ((a + prev[i]) >> 1)) & 0xFF
        elif ft == 4:
            for i in range(stride):
                a = line[i - bpp] if i >= bpp else 0
                c = prev[i - bpp] if i >= bpp else 0
                b = prev[i]
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                pr = a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)
                line[i] = (line[i] + pr) & 0xFF
        elif ft != 0:
            raise ValueError(f'bad filter {ft}')
        out += line
        prev = line
    return bytes(out)


def refilter(pixels, w, h, bpp):
    stride = w * bpp
    out = bytearray()
    prev = bytearray(stride)
    for y in range(h):
        line = pixels[y * stride:(y + 1) * stride]
        best, best_score = None, None
        for ft in range(5):
            cand = bytearray(stride)
            for i in range(stride):
                a = line[i - bpp] if i >= bpp else 0
                b = prev[i]
                c = prev[i - bpp] if i >= bpp else 0
                x = line[i]
                if ft == 0:   v = x
                elif ft == 1: v = x - a
                elif ft == 2: v = x - b
                elif ft == 3: v = x - ((a + b) >> 1)
                else:
                    p = a + b - c
                    pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                    pr = a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)
                    v = x - pr
                cand[i] = v & 0xFF
            score = sum(v if v < 128 else 256 - v for v in cand)
            if best_score is None or score < best_score:
                best, best_score, best_ft = cand, score, ft
        out.append(best_ft)
        out += best
        prev = line
    return bytes(out)


total_before = total_after = 0
for path in sorted(glob.glob('reference/wm2000-frames/*.png')):
    data = open(path, 'rb').read()
    idat = b''
    for typ, body in chunks(data):
        if typ == b'IHDR':
            w, h, depth, ctype = struct.unpack('>IIBB', body[:10])
        elif typ == b'IDAT':
            idat += body
    if depth != 8 or ctype != 6:
        print(f'  skip {os.path.basename(path)}: depth={depth} ctype={ctype}')
        continue
    bpp = 4
    pixels = unfilter(zlib.decompress(idat), w, h, bpp)
    new_raw = refilter(pixels, w, h, bpp)
    # Prove losslessness: decode what we are about to write.
    assert unfilter(new_raw, w, h, bpp) == pixels, f'LOSSY on {path}'
    comp = zlib.compress(new_raw, 9)

    def chunk(typ, body):
        return struct.pack('>I', len(body)) + typ + body + struct.pack(
            '>I', zlib.crc32(typ + body) & 0xFFFFFFFF)

    out = (b'\x89PNG\r\n\x1a\n'
           + chunk(b'IHDR', struct.pack('>IIBBBBB', w, h, 8, 6, 0, 0, 0))
           + chunk(b'IDAT', comp) + chunk(b'IEND', b''))
    total_before += len(data); total_after += len(out)
    open(path, 'wb').write(out)
    print(f'  {os.path.basename(path):38s} {len(data):>7} -> {len(out):>7}'
          f'  ({len(out)/len(data)*100:.1f}%)')

print(f'\ntotal {total_before} -> {total_after} '
      f'({total_after/total_before*100:.1f}%, saved {(total_before-total_after)/1e6:.2f} MB)')
