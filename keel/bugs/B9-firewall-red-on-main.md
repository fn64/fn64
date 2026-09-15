---
name: Answer-key firewall is red on clean main
covers: [C3, C5]
reported: 2026-09-15
status: resolved-by-K21
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

## K21 resolution (2026-09-15)

Three independent causes, not one.

1. **`wrong=1` was a tampered answer key, not a code regression.** The NWXE
   `dump.toml` in the external `aki-recomp` checkout has UNCOMMITTED local
   edits (+40/-11). One splits `func_80038480` (0x1D0) into `func_80038480`
   (0x170) + a new `func_800385F0` (0x60) that the committed key does not
   contain -- but leaves the identical padded boundary 0x20 later, at
   0x80038610, merged. Discovery correctly finds both boundaries, so the
   second grades as a split of the invented symbol. Against the committed
   key, nwxe-donor grades **779 matched, wrong=0, total=847** (847 is also
   the total the fn64-firewall skill documents; the edited key gives 848).
   Nothing in discovery was wrong. Fix: `gate_decomp_functions` now pins the
   answer key's shape (`FN64_DISCOVER_DUMP_FUNCTIONS`/`_SECTIONS`, declared
   not defaulted), so a drifted key fails loudly instead of grading a
   plausible `wrong=1`. The sibling `gate_d1*` gates have done this since
   2026-07-18; the gate that actually owns `wrong == 0` did not.

2. **`gate_overlay_regions` and `gate_d1_overlays` digests were stale.** Both
   moved at 42307ab8 ("feat(corpus): land the static-recomp consolidation
   wave", #119), which taught the descriptor search that a record's third
   field is sometimes the destination EXCLUSIVE END rather than its start.
   The enumerator now proposes each table under both readings and lets
   delta_vote adjudicate, so each real table is listed twice (once
   admitted=true, once admitted=false). Deliberate, tested, and NO graded
   number moved: NW4E 5/5 and NWXE 4/4 still at 100%/100% with 0 WRONG.
   Both digests refreshed with that evidence.

3. **`gate_owners_overlays` is red for a REAL regression (new bug B10).**
   Also first-bad at 42307ab8: block proof now reaches nothing on all four
   recovered overlay banks and exact_owners collapses 46 -> 0. Its digest is
   deliberately NOT re-recorded. This was invisible until (2) was fixed,
   because the stale digests aborted the script before this gate ran.
