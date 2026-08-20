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

# In-match phase, built from the grammar read out of the ROM's own move
# classifier (func_8013ECC0 at 0x8013ECC0, gameplay overlay D2720.s) rather
# than guessed:
#
#   0x8000 A          -> action flag 0x4   weak grapple/strike
#   0x4000 B          -> action flag 0x8   strong grapple/strike
#   0xC000 A|B        -> action flag 0x11  special
#   held >= 8 frames  -> action flag 0x1   CHARGED, a different move
#   0x0F00 D-pad      -> action flag 0x20  directional modifier, OR-ed in
#   0xC030 A|B|L|R    -> the counter/reversal window (func_8013EE44)
#
# The >=8-frame charge rule is the reason the presses here are 3 swaps and not
# the lead-in's 10: a 10-swap hold is not a repeated tap, it is one charge.
# The classifier reads an 8-deep history ring (D_80166EB0, shifted per frame by
# func_8013EBFC), so it wants edge transitions, which the harness's
# feed-on-change seam already produces.
CYCLE = [
    ("0100", 6),   # D-Right: close distance
    ("8000", 3),   # A tap        -> weak strike
    ("4000", 3),   # B tap        -> strong strike
    ("8000", 3),   # A tap
    ("0200", 6),   # D-Left: mirrored pads converge rather than walk together
    ("8100", 3),   # A + D-Right  -> directional variant (flag 0x24)
    ("c000", 10),  # A|B held     -> special (flag 0x11) AND charged (flag 0x1)
    ("4000", 3),   # B tap
    ("0010", 3),   # R            -> counter/reversal window (mask 0xC030)
    ("8000", 3),   # A tap
    ("0004", 3),   # C-Down       -> AKI convention: pin / pick up
    ("8000", 12),  # A held       -> charged grapple (flag 0x1)
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
    ap.add_argument("--port1", action="store_true",
                    help="emit port 1's schedule instead of port 0's: the same "
                         "cycle rotated and phase-shifted, so the two wrestlers "
                         "are not performing identical moves on identical frames")
    ap.add_argument("--gap", type=int, default=10,
                    help="neutral swaps between presses (clears the 4-frame repeat delay)")
    a = ap.parse_args()
    if a.port1:
        # Port 1 gets NO lead-in: the menus are navigated by port 0, and a
        # second pad pressing confirm on the same frames would double-advance
        # them. It joins only for the match, with the cycle rotated (so the two
        # are in different moves) and phase-shifted by half a step (so they are
        # not even on the same frames).
        global CYCLE
        CYCLE = CYCLE[len(CYCLE) // 2:] + CYCLE[: len(CYCLE) // 2]
        print(";".join(inmatch(a.start + 23, a.until, a.gap)))
        return
    parts = leadin()
    if a.mode == "grapple":
        parts += inmatch(a.start, a.until, a.gap)
    print(";".join(parts))

if __name__ == "__main__":
    main()
