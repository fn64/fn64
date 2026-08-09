#!/usr/bin/env python3
"""Compare a run's guest counters against the recorded expectation for its ROUTE.

The route is part of the tuple. The 16586/11005/27591/12008/3115/18776001537 set
belongs to a ~2.1M-step route (perf-method.md:2407); a 1.5M run ends at
11153/7685/18838/8386/2390/13112786076 (:2409, :2610). Checking one against the
other fails and burns the run.

This script never redefines the gate to match the run. A deviation is reported as
a deviation -- on the emulation path that means the emulated program changed.
"""
import re
import sys

expected_path, log_path = sys.argv[1], sys.argv[2]

expected = {}
for line in open(expected_path):
    line = line.split("#", 1)[0].strip()
    if "=" in line:
        k, v = line.split("=", 1)
        expected[k.strip()] = v.strip()

lines = open(log_path, errors="replace").read().splitlines()

# READ THE AUTHORITATIVE LINE, NOT THE LAST TEXTUAL MATCH.
#
# This script originally took the last `key=value` anywhere in the log. That
# manufactured a phantom 303-submit "deviation" and cost four 25-minute runs:
# `[wm2000-block-progress]` reports gfx_submits=11153 (the run total), while a
# LATER line, `[frame-census] steady-state rendering evidence`, reports
# gfx_submits=10850 (the steady-state span, warmup excluded). Both are correct
# and they are different metrics. Last-match-wins silently picked the census
# line and compared a steady-state count against a whole-run expectation.
#
# The guest byte-identity tuple is defined by the progress summary, so anchor
# to it explicitly and fail loudly if it is absent rather than falling back to
# a scan that can pick up a different metric with the same key name.
progress = [l for l in lines if "[wm2000-block-progress]" in l]
if not progress:
    print("FATAL: no [wm2000-block-progress] summary line in this log.\n"
          "Cannot check byte-identity: the counters live on that line and a\n"
          "free-text scan can match a different metric of the same name.",
          file=sys.stderr)
    sys.exit(2)
summary = progress[-1]

# `fields` is NOT on the progress line and is ambiguous by itself: the census
# emits total_fields=8295, transient_fields=595 and a steady-state fields=7699.
# The recorded expectation (7699) is the steady-state figure, so anchor it to
# the line that defines it instead of matching the bare key anywhere.
STEADY_FIELDS_LINE = "steady-state PER-FIELD latency"

# `sim_time` is on the run-completion line, not the progress summary. Heartbeats
# also carry sim_time, so anchor to `done:` -- the end-of-run value is the one
# the tuple means.
DONE_LINE = "] done:"

actual = {}
for key in expected:
    if key == "sim_time":
        for l in lines:
            if DONE_LINE in l:
                m = re.search(r"\bsim_time=([0-9]+)", l)
                if m:
                    actual[key] = m.group(1)
                break
        continue
    if key == "fields":
        for l in lines:
            if STEADY_FIELDS_LINE in l:
                m = re.search(r"\bfields=([0-9]+)", l)
                if m:
                    actual[key] = m.group(1)
                break
        continue
    m = re.search(rf"\b{re.escape(key)}=([A-Za-z0-9_]+)", summary)
    if m:
        actual[key] = m.group(1)

width = max(len(k) for k in expected)
ok = missing = bad = 0
print(f"{'counter':<{width}}  {'expected':>14}  {'actual':>14}  verdict")
print("-" * (width + 42))
for k, want in expected.items():
    got = actual.get(k)
    if got is None:
        verdict, missing = "NOT FOUND", missing + 1
    elif got == want:
        verdict, ok = "match", ok + 1
    else:
        verdict, bad = "*** DEVIATION ***", bad + 1
    print(f"{k:<{width}}  {want:>14}  {str(got):>14}  {verdict}")

total = len(expected)
print(f"\n{ok} of {total} match, {bad} deviate, {missing} not found in log")
if bad:
    print("\nGUEST BYTE-IDENTITY FAILED. The emulated program changed; it does not ship.")
elif missing:
    print("\nINCONCLUSIVE: counters absent from the log. Did the run reach the summary?")
else:
    print("\nGUEST BYTE-IDENTICAL for this route.")
sys.exit(0 if (ok == total) else 1)
