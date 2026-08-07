#!/usr/bin/env python3
"""Who calls the hot leaf? Caller attribution for an xctrace cpu-profile export.

`wm2000_self_time.py` answers "what is slow" (leaf frame, weighted). It cannot
answer "why is it called", and that is the question a structural optimization
needs: `RdramView::read_u8` at 11% self time is only actionable once you know
whether it is one call site in a loop or ten thousand scattered guest loads.

Same two rules as the self-time script, for the same reasons:

1. The leaf is the INNERMOST frame (`frames[0]`). Callers are the frames
   outward from it, in order.
2. Each run is slid independently; convert every main-image address with that
   run's own `load-addr` from its own export.

Usage:
    wm2000_callers.py --dsym PATH --symbol 'read_u8' run-1.xml [run-2.xml ...]
    wm2000_callers.py --dsym PATH --leaves run-1.xml        # leaf histogram
    wm2000_callers.py --selftest
"""

from __future__ import annotations

import argparse
import collections
import subprocess
import sys

try:
    import defusedxml.ElementTree as ET
except ImportError:  # pragma: no cover - environment-dependent
    import xml.etree.ElementTree as ET

BASE = 0x100000000


def _looks_like_bare_address(name):
    stripped = (name or "").strip()
    if not stripped.startswith("0x"):
        return False
    try:
        int(stripped, 16)
    except ValueError:
        return False
    return True


def parse_stacks(path):
    """Return ([(weight, [(addr, name, binary_name)...])], main_image, slide).

    Frames are innermost-first, matching the export's own order.
    """
    root = ET.parse(path).getroot()

    by_id = {}
    for element in root.iter():
        element_id = element.get("id")
        if element_id is not None:
            by_id[element_id] = element

    def deref(node):
        if node is None:
            return None
        ref = node.get("ref")
        if ref is not None:
            return by_id.get(ref)
        return node

    stacks = []
    image_weights = collections.Counter()
    slide_by_image = {}

    for row in root.iter("row"):
        weight = 0
        backtrace = None
        for child in row:
            if child.tag == "cycle-weight":
                node = deref(child)
                if node is not None and node.text:
                    try:
                        weight = int(node.text.strip())
                    except ValueError:
                        pass
            elif child.tag == "tagged-backtrace":
                tagged = deref(child)
                if tagged is not None:
                    backtrace = tagged.find("backtrace")
        if backtrace is None:
            continue

        frames = []
        for raw in backtrace.iter("frame"):
            frame = deref(raw)
            if frame is None:
                continue
            binary = deref(frame.find("binary"))
            binary_name = binary.get("name") if binary is not None else None
            if binary is not None and binary.get("load-addr") and binary_name:
                try:
                    slide_by_image.setdefault(binary_name, int(binary.get("load-addr"), 16))
                except ValueError:
                    pass
            name = frame.get("name")
            if _looks_like_bare_address(name):
                name = None
            frames.append((frame.get("addr"), name, binary_name))
        if not frames:
            continue
        stacks.append((weight, frames))
        leaf_binary = frames[0][2]
        if leaf_binary:
            image_weights[leaf_binary] += weight

    main_image = image_weights.most_common(1)[0][0] if image_weights else None
    return stacks, main_image, slide_by_image.get(main_image)


def resolve(addrs, dsym):
    if not addrs:
        return {}
    proc = subprocess.run(
        ["atos", "-o", dsym, "-l", hex(BASE)] + [hex(a) for a in sorted(addrs)],
        capture_output=True, text=True,
    )
    out = [line.strip() for line in proc.stdout.splitlines()]
    return dict(zip(sorted(addrs), out))


def label_stacks(paths, dsym):
    """Yield (weight, [resolved frame labels innermost-first]) across runs."""
    labelled = []
    for path in paths:
        stacks, main_image, slide = parse_stacks(path)
        wanted = set()
        for _weight, frames in stacks:
            for addr, name, binary_name in frames:
                if name or addr is None or slide is None or binary_name != main_image:
                    continue
                try:
                    wanted.add(int(addr, 16) - slide + BASE)
                except ValueError:
                    pass
        table = resolve(wanted, dsym)
        for weight, frames in stacks:
            labels = []
            for addr, name, binary_name in frames:
                if name:
                    labels.append(name)
                    continue
                if addr is None or slide is None or binary_name != main_image:
                    labels.append(f"<{binary_name or 'unresolved'}>")
                    continue
                try:
                    key = int(addr, 16) - slide + BASE
                except ValueError:
                    labels.append("<unresolved>")
                    continue
                labels.append(table.get(key, "<unresolved>"))
            labelled.append((weight, labels))
    return labelled


def main(argv=None):
    ap = argparse.ArgumentParser()
    ap.add_argument("--dsym")
    ap.add_argument("--symbol", help="substring; report callers of the innermost match")
    ap.add_argument("--leaves", action="store_true", help="leaf self-time histogram")
    ap.add_argument("--depth", type=int, default=6, help="caller levels to report")
    ap.add_argument("--top", type=int, default=15)
    ap.add_argument("--selftest", action="store_true")
    ap.add_argument("exports", nargs="*")
    args = ap.parse_args(argv)

    if args.selftest:
        return selftest()
    if not args.dsym or not args.exports:
        ap.error("--dsym and at least one export are required")

    labelled = label_stacks(args.exports, args.dsym)
    total = sum(w for w, _ in labelled)
    if total == 0:
        print("no samples parsed", file=sys.stderr)
        return 1

    if args.leaves or not args.symbol:
        hist = collections.Counter()
        for weight, labels in labelled:
            hist[labels[0]] += weight
        print(f"\nLEAF self time, {total:,} weighted samples")
        for name, weight in hist.most_common(args.top):
            print(f"{100*weight/total:7.2f}%  {weight:12,}  {name}")
        return 0

    # Callers of the innermost frame matching --symbol. Inclusive of that
    # frame: a sample counts if the symbol appears anywhere on the stack, and
    # the caller is the next frame outward from its innermost occurrence.
    matched = 0
    at_leaf = 0
    levels = [collections.Counter() for _ in range(args.depth)]
    for weight, labels in labelled:
        index = next((i for i, l in enumerate(labels) if args.symbol in l), None)
        if index is None:
            continue
        matched += weight
        if index == 0:
            at_leaf += weight
        for depth in range(args.depth):
            caller = labels[index + 1 + depth] if index + 1 + depth < len(labels) else "<top>"
            levels[depth][caller] += weight

    print(f"\nCallers of frames matching {args.symbol!r}")
    print(f"on-stack (inclusive): {matched:,} ({100*matched/total:.2f}% of {total:,})")
    print(f"as leaf (self):       {at_leaf:,} ({100*at_leaf/total:.2f}%)")
    for depth, hist in enumerate(levels):
        print(f"\n-- caller +{depth+1}")
        for name, weight in hist.most_common(args.top):
            print(f"{100*weight/matched:7.2f}%  {weight:12,}  {name}")
    return 0


def selftest():
    import tempfile, os
    failures = []

    def check(label, cond):
        print(f"  {'ok  ' if cond else 'FAIL'} {label}")
        if not cond:
            failures.append(label)

    xml = """<trace-query-result><node><table schema="cpu-profile">
      <row>
        <cycle-weight id="9">100</cycle-weight>
        <tagged-backtrace id="10"><backtrace id="11">
          <frame id="12" name="leaf_fn" addr="0x102001000">
            <binary id="22" name="wm2000-block-boot" load-addr="0x102000000"/>
          </frame>
          <frame id="13" name="caller_one" addr="0x102002000"><binary ref="22"/></frame>
          <frame id="14" name="caller_two" addr="0x102003000"><binary ref="22"/></frame>
        </backtrace></tagged-backtrace>
      </row>
      <row>
        <cycle-weight ref="9"/>
        <tagged-backtrace id="24"><backtrace id="25">
          <frame id="26" name="other_leaf" addr="0x102009000"><binary ref="22"/></frame>
          <frame id="27" name="leaf_fn" addr="0x102001000"><binary ref="22"/></frame>
          <frame id="28" name="caller_three" addr="0x10200a000"><binary ref="22"/></frame>
        </backtrace></tagged-backtrace>
      </row>
    </table></node></trace-query-result>"""

    with tempfile.NamedTemporaryFile("w", suffix=".xml", delete=False) as fh:
        fh.write(xml)
        path = fh.name
    try:
        stacks, main_image, slide = parse_stacks(path)
        check("parses every row", len(stacks) == 2)
        check("identifies the main image", main_image == "wm2000-block-boot")
        check("reads the load address", slide == 0x102000000)
        check("keeps frames innermost-first", stacks[0][1][0][1] == "leaf_fn")
        check("follows a cycle-weight ref", stacks[1][0] == 100)
        check("follows nested binary refs",
              all(f[2] == "wm2000-block-boot" for _w, fs in stacks for f in fs))

        # The caller rule: leaf_fn appears in both rows -- once as the leaf,
        # once one frame in. Its caller is the next frame OUTWARD in each.
        labelled = [(w, [f[1] for f in fs]) for w, fs in stacks]
        callers = collections.Counter()
        for weight, labels in labelled:
            index = next(i for i, l in enumerate(labels) if l == "leaf_fn")
            callers[labels[index + 1]] += weight
        check("attributes each occurrence to its own caller",
              callers == collections.Counter({"caller_one": 100, "caller_three": 100}))
        # And self time counts only the row where it is on top.
        check("self time is the leaf occurrence only",
              sum(w for w, ls in labelled if ls[0] == "leaf_fn") == 100)
    finally:
        os.unlink(path)

    print()
    if failures:
        print(f"selftest: {len(failures)} failure(s)", file=sys.stderr)
        return 1
    print("selftest: all checks passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
