#!/usr/bin/env python3
"""Aggregate xctrace CPU-profile exports into a SELF-time table.

Two rules, both learned by getting them wrong (see
docs/plans/resolvable-self-time-profile.md):

1. SELF TIME IS THE LEAF FRAME ONLY, weighted by the row's cycle-weight.
   Reading inclusive totals as self time caused three consecutive failed
   optimizations in this repository. A frame's self time is the weight of the
   samples where it is on TOP of the stack -- not the weight of samples where
   it appears anywhere.

2. EACH RUN IS SLID INDEPENDENTLY. macOS ASLR gives every process a different
   load address, so a main-image frame's address must be converted with the
   `load-addr` from ITS OWN export before it means anything. A shared or
   inferred slide produced a top-15 list led by `PathBuf::__set_extension` in a
   loop that touches no paths, and a symbol-proximity fit scored 88% while
   still being off by 0x3c000. The `load-addr` attribute in the export is
   ground truth; nothing else is.

Usage:
    wm2000_self_time.py --dsym PATH run-1.xml [run-2.xml ...]
    wm2000_self_time.py --selftest
"""

from __future__ import annotations

import argparse
import collections
import subprocess
import sys

# The exports are produced locally by `xctrace` from our own traces, so this is
# not an untrusted-input path -- but the stdlib parser resolves external
# entities and expands nested entities, so a malformed or hostile trace could
# hang or read arbitrary files. defusedxml costs nothing here; fall back only
# if it is unavailable, since requiring it would make the profiler unrunnable
# on a machine that is otherwise ready.
try:
    import defusedxml.ElementTree as ET
except ImportError:  # pragma: no cover - environment-dependent
    import xml.etree.ElementTree as ET
    print("note: defusedxml not installed; using the stdlib XML parser "
          "(fine for locally-generated xctrace exports)", file=sys.stderr)

# atos is invoked with -l 0x100000000, so every address is normalised into that
# space before lookup: addr - load_addr + BASE.
BASE = 0x100000000


def parse_export(path):
    """Return ([(addr, name, weight, binary_name, binary_load_addr)], main_image).

    Written against the schema `xctrace export` actually emits, verified
    against a real capture rather than assumed:

    - The document root is `trace-query-result`, not `trace-toc`.
    - Every element is id-tagged on first appearance and referenced by `ref`
      afterwards. Following refs is mandatory: a single row defines the thread
      and process, and the other 1,470 only reference them.
    - A row's stack lives at `tagged-backtrace > backtrace > frame`, innermost
      frame FIRST.
    - `binary` is nested INSIDE `frame`, not a sibling of it, and only the
      frames whose image is symbolicated carry one at all.
    - An unresolved frame still has a `name` -- set to the hex address, e.g.
      `name="0x109f14af9"`. Treating that as a resolved symbol produces a
      profile of meaningless hex, which is one of the two wrong profiles the
      module docstring warns about. Such names are rejected here.
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

    rows = []
    image_weights = collections.Counter()

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
        frames = list(backtrace.iter("frame"))
        if not frames:
            continue
        leaf = deref(frames[0])
        if leaf is None:
            continue

        binary = deref(leaf.find("binary"))
        binary_name = binary.get("name") if binary is not None else None
        load_addr = None
        if binary is not None and binary.get("load-addr"):
            try:
                load_addr = int(binary.get("load-addr"), 16)
            except ValueError:
                load_addr = None

        name = leaf.get("name")
        # Reject the "resolved to its own address" pseudo-name.
        if name is not None and _looks_like_bare_address(name):
            name = None

        rows.append((leaf.get("addr"), name, weight, binary_name, load_addr))
        if binary_name:
            image_weights[binary_name] += weight

    main_image = image_weights.most_common(1)[0][0] if image_weights else None
    return rows, main_image


def _looks_like_bare_address(name):
    """True for names xctrace synthesises from an unresolved address."""
    stripped = name.strip()
    if not stripped.startswith("0x"):
        return False
    try:
        int(stripped, 16)
    except ValueError:
        return False
    return True


def resolve_symbols(addrs, dsym):
    """Batch-resolve normalised addresses through atos."""
    if not addrs:
        return {}
    proc = subprocess.run(
        ["atos", "-o", dsym, "-l", hex(BASE)] + [hex(a) for a in addrs],
        capture_output=True, text=True,
    )
    out = [line.strip() for line in proc.stdout.splitlines()]
    return dict(zip(addrs, out))


def aggregate(paths, dsym):
    self_time = collections.Counter()
    total = 0
    per_run_slides = []

    for path in paths:
        rows, main_image = parse_export(path)

        # One slide per run, read from THIS run's own export. Never inferred:
        # a fitted slide scored 88% and was still off by 0x3c000.
        main_slide = None
        for _addr, _name, _weight, binary_name, load_addr in rows:
            if binary_name == main_image and load_addr is not None:
                main_slide = load_addr
                break
        per_run_slides.append(main_slide)

        pending = []
        for addr, name, weight, binary_name, _load_addr in rows:
            total += weight
            if name:
                self_time[name] += weight
                continue
            # Only main-image addresses can be resolved through our dSYM;
            # a system-library address run through it would yield a confident
            # and completely wrong symbol.
            if addr is None or main_slide is None or binary_name != main_image:
                label = f"<{binary_name}>" if binary_name else "<unresolved>"
                self_time[label] += weight
                continue
            try:
                a = int(addr, 16)
            except ValueError:
                self_time["<unresolved>"] += weight
                continue
            pending.append((a - main_slide + BASE, weight))

        if pending:
            uniq = sorted({a for a, _ in pending})
            table = resolve_symbols(uniq, dsym)
            for a, weight in pending:
                self_time[table.get(a, "<unresolved>")] += weight

    return self_time, total, per_run_slides


def main(argv=None):
    ap = argparse.ArgumentParser()
    ap.add_argument("--dsym")
    ap.add_argument("--top", type=int, default=20)
    ap.add_argument("--selftest", action="store_true")
    ap.add_argument("exports", nargs="*")
    args = ap.parse_args(argv)

    if args.selftest:
        return selftest()

    if not args.dsym or not args.exports:
        ap.error("--dsym and at least one export are required")

    self_time, total, slides = aggregate(args.exports, args.dsym)
    if total == 0:
        print("no samples parsed -- the export schema may have changed", file=sys.stderr)
        return 1

    # Distinct slides per run is the EXPECTED state under ASLR. If every run
    # reports the same slide, either ASLR is off or we failed to read per-run
    # load-addrs -- and the second is the bug that produced a wrong profile, so
    # say so rather than printing a confident table.
    if len(args.exports) > 1 and len(set(slides)) == 1 and slides[0] is not None:
        print(f"WARNING: all {len(args.exports)} runs report the same load address "
              f"({hex(slides[0])}). Under ASLR that is unexpected; the per-run "
              f"slide may not have been read. Treat this profile as suspect.",
              file=sys.stderr)

    unresolved = self_time.get("<unresolved>", 0)
    print(f"\nSELF time, {len(args.exports)} run(s), {total:,} weighted samples")
    print("NOTE: symbol NAMES are reliable; inlined-frame LINE NUMBERS and the")
    print("      attribution of heavily-inlined helpers are not. Verify any")
    print("      surprising attribution with `nm` or a counter before acting:")
    print("      a run of this profiler attributed 6.21% to a #[cfg(test)]")
    print("      function that `nm` proves is absent from the binary.")
    print(f"per-run load addresses: {[hex(s) if s else None for s in slides]}")
    if unresolved:
        print(f"unresolved: {unresolved:,} ({100*unresolved/total:.2f}%)")
    print()
    print(f"{'SELF%':>8}  {'weight':>12}  symbol")
    for name, weight in self_time.most_common(args.top):
        print(f"{100*weight/total:7.2f}%  {weight:12,}  {name}")
    return 0


def selftest():
    """The parser and the self-time rule, checked on a synthetic export."""
    import tempfile, os
    failures = []

    def check(label, cond):
        print(f"  {'ok  ' if cond else 'FAIL'} {label}")
        if not cond:
            failures.append(label)

    # Shaped exactly like a real `xctrace export`, verified against a capture:
    # trace-query-result root, tagged-backtrace wrapper, binary nested inside
    # frame, id/ref deduplication, and an unresolved frame whose `name` is its
    # own hex address.
    xml = """<trace-query-result><node><table schema="cpu-profile">
      <row>
        <cycle-weight id="9">100</cycle-weight>
        <tagged-backtrace id="10"><backtrace id="11">
          <frame id="12" name="0x102001000" addr="0x102001000">
            <binary id="22" name="wm2000-block-boot" load-addr="0x102000000"/>
          </frame>
          <frame id="13" name="0x102002000" addr="0x102002000"><binary ref="22"/></frame>
        </backtrace></tagged-backtrace>
      </row>
      <row>
        <cycle-weight id="18">50</cycle-weight>
        <tagged-backtrace id="19"><backtrace id="20">
          <frame id="21" name="real_symbol(int)" addr="0x18c8d7289">
            <binary id="23" name="dyld" load-addr="0x18c868000"/>
          </frame>
        </backtrace></tagged-backtrace>
      </row>
      <row>
        <cycle-weight ref="9"/>
        <tagged-backtrace id="24"><backtrace id="25">
          <frame id="26" name="0x102009000" addr="0x102009000"><binary ref="22"/></frame>
        </backtrace></tagged-backtrace>
      </row>
    </table></node></trace-query-result>"""

    with tempfile.NamedTemporaryFile("w", suffix=".xml", delete=False) as fh:
        fh.write(xml)
        path = fh.name
    try:
        rows, main_image = parse_export(path)
        check("parses every row", len(rows) == 3)
        # Two of the three rows are main-image, so it wins on weight.
        check("identifies the main image by weight", main_image == "wm2000-block-boot")
        check("follows a cycle-weight ref", sorted(r[2] for r in rows) == [50, 100, 100])
        check("reads the nested binary's load address",
              any(r[4] == 0x102000000 for r in rows))
        check("follows a nested binary ref",
              sum(1 for r in rows if r[3] == "wm2000-block-boot") == 2)
        # The LEAF is the innermost (first) frame. Getting this backwards is
        # the inclusive-vs-self error in parser form.
        check("takes the innermost frame as the leaf",
              rows[0][0] == "0x102001000")
        # A hex-address `name` must NOT be mistaken for a resolved symbol.
        check("rejects a bare-address pseudo-name", rows[0][1] is None)
        check("keeps a genuinely resolved name", rows[1][1] == "real_symbol(int)")
        check("_looks_like_bare_address discriminates",
              _looks_like_bare_address("0x109f14af9")
              and not _looks_like_bare_address("real_symbol(int)")
              and not _looks_like_bare_address("0xdeadbeef_thing"))
        # Normalisation must subtract THIS run's slide.
        check("normalises against the run's own slide",
              0x102001000 - 0x102000000 + BASE == 0x100001000)
    finally:
        os.unlink(path)

    print()
    if failures:
        print(f"selftest: {len(failures)} failure(s)", file=sys.stderr)
        return 1
    print("selftest: all checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
