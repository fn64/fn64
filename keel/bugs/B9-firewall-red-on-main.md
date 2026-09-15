---
name: Answer-key firewall is red on clean main
covers: [C3, C5]
reported: 2026-09-15
---
repro: on main-equivalent rev 6306c9d0 with K18 fully reverted, source the
canonical env and run scripts/grade-all.sh and scripts/gate-determinism.sh
expected: every config wrong=0 at the README recall figures; every
determinism gate at its recorded digest
actual: nwxe-donor 779 matched with wrong=1; nwxe-solo 726 (README 725);
nw4e-donor 925/0, nw4e-solo 873/0, revenge-solo 597/0 hold.
gate_overlay_regions digest dc7d29a8 vs recorded 471181f2, which aborts the
script before expected_closure (gate-closure measured directly: 765e6349,
unchanged by K18). Reproduced byte-identically with and without K18.
note: wrong>0 is disqualifying by the firewall rule; this predates the
corpus-funnel branch and needs its own bisect on main.
