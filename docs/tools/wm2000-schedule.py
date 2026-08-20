#!/usr/bin/env python3
"""Generate a WM2000_INPUT_SCRIPT for a LONG in-match run.

The committed lead-in (docs/tools/wm2000-match-leadin.txt) stops issuing
presses at swap 6000, which is fine for "does the ROM reach a match" and wrong
for "does the match END": a wrestling match with two idle pads may sit on an
idle timer forever. This extends the same lead-in with a repeating in-match
phase and, critically, makes that phase EASY TO VARY so a differential can be
driven (identical lead-in, different in-match tail).

The harness mirrors one composed pad state onto every plugged port
(WM2000_PORTS), so both wrestlers receive the SAME buttons. That is a real
limitation of the seam, not of this generator, and it is why the in-match
phase deliberately alternates directions: two identical pads pressing the same
direction walk together, two pads alternating still collide.

Buttons are the N64 OSContPad.button bits:
  A=8000 B=4000 Z=2000 START=1000 DU=0800 DD=0400 DL=0200 DR=0100
  L=0020 R=0010 CU=0008 CD=0004 CL=0002 CR=0001

Usage:
  wm2000-schedule.py --mode leadin-only          # presses stop at 6000 (control)
  wm2000-schedule.py --mode grapple --until 60000
"""
import argparse

# The committed lead-in, regenerated rather than read, so the two phases share
# one definition of "where the match starts".
def leadin():
    out = ["1100..1110:1000"]                       # START
    for s in range(1200, 2500, 100):                # A every 100 to 2400
        out.append(f"{s}..{s+10}:8000")
    for s in range(2500, 6000, 60):                 # A every 60 to 5980
        out.append(f"{s}..{s+10}:8000")
    return out

# In-match phase. A cycle of moves rather than a single mashed button: walk,
# grapple, strike, and (per the AKI convention) a C-button attempt, so that if
# any one of them is the move/pin button the schedule contains it.
CYCLE = [
    ("0100", 20),  # D-Right: walk toward
    ("8000", 10),  # A: weak grapple / strike
    ("0200", 20),  # D-Left: walk toward (opposite, so mirrored pads converge)
    ("4000", 10),  # B: strong grapple / strike
    ("8000", 10),  # A again (grapple follow-up)
    ("0004", 10),  # C-Down: AKI convention pin / pick up
    ("0001", 10),  # C-Right
    ("0010", 10),  # R: run / block
]

def inmatch(start, until, gap):
    out, s = [], start
    while s < until:
        for buttons, hold in CYCLE:
            if s >= until:
                break
            out.append(f"{s}..{s+hold}:{buttons}")
            s += hold + gap
    return out

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--mode", choices=["leadin-only", "grapple"], default="grapple")
    ap.add_argument("--until", type=int, default=60000)
    ap.add_argument("--start", type=int, default=6000)
    ap.add_argument("--gap", type=int, default=10,
                    help="neutral swaps between presses (clears the 4-frame repeat delay)")
    a = ap.parse_args()
    parts = leadin()
    if a.mode == "grapple":
        parts += inmatch(a.start, a.until, a.gap)
    print(";".join(parts))

if __name__ == "__main__":
    main()
