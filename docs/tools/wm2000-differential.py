#!/usr/bin/env python3
"""Compare two WM2000 run logs and say whether in-match input changed anything.

The question "does input reach gameplay" has been answered wrongly twice on
this ROM by comparing framebuffers. The frame hash is the documented wrong
instrument here: every wave-1 probe looked equally stuck by hash while the
gfx-task rate showed one had left the idle state and stayed out, and a screen
can compose at full rate with a frozen scanned-out image.

So this compares, in order of how directly each bears on the question:

  1. GUEST MEMORY. The ROM's own gameplay input record (D_80095180, stride
     0xC) and match state machine (D_801589D6). If the treatment run's input
     words change and the control's do not, the button provably arrived where
     the game reads it -- no rendering involved.
  2. MATCH STATE. Whether either run left state 2 (live match) for 3
     (decision) or 4 (post-match), and what the end flags say.
  3. GFX-TASK RATE. The secondary signal, kept because it is the one that has
     historically distinguished runs the frame hash could not.

Usage:  wm2000-differential.py <treatment.log> <control.log>
"""
import argparse
import pathlib
import re
import sys

WATCH = re.compile(r"\[wm2000-watch\] swap #(\d+): (0x[0-9a-f]+) = (0x[0-9a-f]+)")
PROG = re.compile(r"vi_swaps=(\d+) gfx_tasks=(\d+) audio_tasks=(\d+)")

STATE_NAMES = {0: "init", 1: "entrance", 2: "LIVE MATCH", 3: "DECISION", 4: "post-match"}
LABELS = {
    "0x80095184": "port0 HELD", "0x80095186": "port0 PRESSED",
    "0x80095190": "port1 HELD", "0x80095192": "port1 PRESSED",
    "0x801589d6": "MATCH STATE", "0x8016ed2a": "end flags",
    "0x801589d2": "post-match counter", "0x801589d4": "winner index",
    "0x8016f0ac": "match clock", "0x80166f88": "match clock (2)",
    "0x8016ecc0": "referee count",
    "0x801589e6": "PIN COUNT", "0x801589e4": "pin count target",
    "0x801671f0": "P0 spirit", "0x801672f4": "P1 spirit",
    "0x800961d2": "time-limit setting", "0x8014e1c4": "time-limit table[0]",
}


def parse(path):
    text = pathlib.Path(path).read_text(errors="replace")
    watch = {}
    for swap, addr, val in WATCH.findall(text):
        watch.setdefault(addr, []).append((int(swap), int(val, 16)))
    prog = [(int(s), int(g), int(a)) for s, g, a in PROG.findall(text)]
    last = re.findall(r"vi_swaps=(\d+)", text)
    term = "UNKNOWN -- run may have been killed"
    for marker, name in (("step budget", "step budget exhausted"),
                         ("steady idle state", "steady idle state"),
                         ("BOOT SUMMARY", "BOOT SUMMARY"),
                         ("WM2000_STOP_AT_SWAP", "STOP_AT_SWAP")):
        if marker in text:
            term = name
    return {"watch": watch, "prog": prog, "swaps": int(last[-1]) if last else 0,
            "term": term, "panics": text.count("panicked at"),
            "backend": text.count("backend error")}


def rate(prog, lo, hi):
    """gfx tasks per swap over a FIXED window, so unequal-length runs compare."""
    pts = [(s, g) for s, g, _ in prog if lo <= s <= hi]
    if len(pts) < 2:
        return None
    (s0, g0), (s1, g1) = pts[0], pts[-1]
    return (g1 - g0) / (s1 - s0) if s1 > s0 else None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("treatment")
    ap.add_argument("control")
    a = ap.parse_args()
    t, c = parse(a.treatment), parse(a.control)

    for name, r in (("treatment", t), ("control", c)):
        print(f"{name:10s} reached swap {r['swaps']:>7}  terminated: {r['term']}"
              f"  panics={r['panics']} backend_errors={r['backend']}")

    print("\n-- 1. guest memory: did the button reach where the game reads it? --")
    verdict_input = False
    for addr in ("0x80095184", "0x80095186", "0x80095190", "0x80095192"):
        nt, nc = len(t["watch"].get(addr, [])), len(c["watch"].get(addr, []))
        # Count only changes AFTER the lead-in ends, which is the only window
        # in which the two runs are supposed to differ at all.
        at = sum(1 for s, _ in t["watch"].get(addr, []) if s >= 6000)
        ac = sum(1 for s, _ in c["watch"].get(addr, []) if s >= 6000)
        if at > ac:
            verdict_input = True
        print(f"  {LABELS.get(addr, addr):16s} total t={nt:>5} c={nc:>5} | after swap 6000: t={at:>5} c={ac:>5}")
    print("  => " + ("CONFIRMED: in-match input reaches the gameplay input record"
                     if verdict_input else
                     "NOT SHOWN: the treatment's gameplay input words did not change more than the control's"))

    print("\n-- 2. match state machine --")
    for name, r in (("treatment", t), ("control", c)):
        hist = r["watch"].get("0x801589d6", [])
        if not hist:
            print(f"  {name:10s} MATCH STATE never observed changing")
        else:
            path = " -> ".join(f"{v}({STATE_NAMES.get(v, '?')})@{s}" for s, v in hist)
            print(f"  {name:10s} {path}")
            if any(v >= 3 for _, v in hist):
                print(f"  {'':10s} *** REACHED THE DECISION STATE -- THE MATCH ENDED ***")
    for addr in ("0x8016ed2a", "0x801589d4", "0x801589e6", "0x8016ecc0",
                 "0x8016f0ac", "0x800961d2", "0x8014e1c4"):
        for name, r in (("treatment", t), ("control", c)):
            hist = r["watch"].get(addr, [])
            if hist:
                print(f"  {LABELS.get(addr, addr):20s} {name:10s} last={hist[-1][1]:#x} @swap {hist[-1][0]} ({len(hist)} changes)")

    for name, r in (("treatment", t), ("control", c)):
        pins = [(s_, v) for s_, v in r["watch"].get("0x801589e6", []) if v]
        if pins:
            print(f"  *** {name}: PIN IN PROGRESS -- count reached {max(v for _, v in pins)} "
                  f"(first at swap {pins[0][0]}) ***")

    print("\n-- 3. gfx-task rate (secondary; the frame hash is the wrong instrument here) --")
    hi = min(t["swaps"], c["swaps"])
    for lo, label in ((0, "whole run"), (6000, "after the lead-in")):
        if hi > lo:
            rt, rc = rate(t["prog"], lo, hi), rate(c["prog"], lo, hi)
            fmt = lambda x: f"{x:.2f}" if x is not None else "  n/a"
            print(f"  {label:18s} swaps {lo}..{hi}: treatment {fmt(rt)}  control {fmt(rc)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
