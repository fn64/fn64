#!/usr/bin/env python3
"""Report per-swap graphics-task rate from WM2000 harness logs.

Why this exists: the frame hash is the obvious instrument for "did the screen
change", and on this ROM it is the wrong one. Every wave-1 probe looked
equally stuck by hash, while the gfx-task rate showed one of them had left the
idle state and stayed out (see RT64-WM2000-INPUT-GRAMMAR.md).

The plateau signature is a collapse from ~3.0 display lists per field to
exactly 1.00 -- one static list being re-presented. A run that recovers to 3.00
is composing again, whatever its framebuffer says.

Rates are computed over a FIXED swap window by interpolating the cumulative
counters, so runs of unequal length stay comparable; comparing raw tail
averages would credit a longer run for nothing but running longer.
"""
import argparse, pathlib, re

def points(log):
    """(vi_swaps, gfx_tasks, audio_tasks) from the harness progress lines."""
    t = pathlib.Path(log).read_text(errors="replace")
    return [(int(s), int(g), int(a)) for s, g, a in re.findall(
        r"vi_swaps=(\d+) gfx_tasks=(\d+) audio_tasks=(\d+)", t)]

def cumulative_at(pts, swap, idx):
    """Counter value at `swap`, linearly interpolated between samples."""
    prev = None
    for p in pts:
        if p[0] >= swap:
            if prev is None:
                return float(p[idx])
            if p[0] == prev[0]:
                return float(p[idx])
            f = (swap - prev[0]) / (p[0] - prev[0])
            return prev[idx] + (p[idx] - prev[idx]) * f
        prev = p
    return None

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("logs", nargs="+")
    ap.add_argument("--from-swap", type=int, default=3400)
    ap.add_argument("--to-swap", type=int, default=4100)
    ap.add_argument("--profile", action="store_true",
                    help="print the whole per-sample rate profile too")
    a = ap.parse_args()
    span = a.to_swap - a.from_swap
    assert span > 0, "--to-swap must exceed --from-swap"

    for log in a.logs:
        pts = points(log)
        name = pathlib.Path(log).stem
        if not pts:
            print(f"{name:14s} no progress lines"); continue
        mx = pts[-1][0]
        if mx < a.to_swap:
            print(f"{name:14s} max swap {mx} < window end {a.to_swap} "
                  f"-- NOT COMPARABLE, not reporting a rate")
            continue
        g0, g1 = cumulative_at(pts, a.from_swap, 1), cumulative_at(pts, a.to_swap, 1)
        d0, d1 = cumulative_at(pts, a.from_swap, 2), cumulative_at(pts, a.to_swap, 2)
        print(f"{name:14s} swaps {a.from_swap}-{a.to_swap}: "
              f"gfx/swap {(g1-g0)/span:.2f}  audio/swap {(d1-d0)/span:.2f}  "
              f"(max swap {mx})")
        if a.profile:
            for i in range(1, len(pts)):
                ds = pts[i][0] - pts[i-1][0]
                if ds > 0:
                    print(f"    swaps {pts[i][0]:6d}  "
                          f"gfx/swap {(pts[i][1]-pts[i-1][1])/ds:.2f}")

if __name__ == "__main__":
    main()
