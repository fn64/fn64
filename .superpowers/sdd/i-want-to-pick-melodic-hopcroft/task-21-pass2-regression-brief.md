# Task 21: root-cause + fix the Pass 2 corpus regression, then commit Pass 2

## Situation
Fan-out Pass 2 added ~32 new parity cases to the parity runner
(`crates/fn64-render-conformance/src/bin/fn64-render-conformance-parity-runner.rs`)
but the integrator agent HUNG before committing or verifying. Its edits are
UNCOMMITTED in the working tree right now (git status shows the runner modified,
+1265/-89 lines). The corpus is valuable (70 pass-all-match-hardware, 0 fn64-defect,
useful new refusals + shared divergences) BUT it introduced a REGRESSION that must
be fixed before committing.

## The regression (already localized — do not re-derive from scratch)
Three PRE-EXISTING hand-derived cases now FAIL the gate:
`textured-rect-line17-low-t95`, `textured-rect-line17-low-t94`,
`textured-rect-line16-low-t95`. Each reports `verdict=identical` (wgpu==RT64) but
`wgpu_matches_key=False` — wgpu no longer matches its hand-derived key.

PROVEN facts:
- BEFORE Pass 2 (a clean gate run captured earlier) these three had
  `wgpu_matches_key=True`. AFTER the Pass 2 edits they are `False`. So Pass 2 caused it.
- It is DETERMINISTIC (fails on repeated runs) and reproduces even with
  `FN64_ONLY=textured-rect-line` (only these cases run) — so it is NOT a
  case-ordering / GPU-state-leak between cases.
- The render-wgpu SOURCE is untouched (only the runner + scripts/check_rt64_parity.py
  are dirty), so the actual rendered pixels cannot have changed. `matches_key=False`
  with unchanged pixels means the HAND-DERIVED KEY these cases compare against
  shifted — i.e. a Pass 2 addition changed a shared constant / texel table / tile
  setup / key-computation helper that these `low-t` texrect cases consume.
- The runner diff is almost entirely additions; the only modification to existing
  code is the `generated_cases()` closure signature line. So look for a NEWLY ADDED
  `const`/`static`/helper whose NAME SHADOWS or whose VALUE is read by the existing
  `textured-rect-line*-low-t*` case builders or their expected-key function.
- `scripts/check_rt64_parity.py` is also dirty but its only change is the removal of
  the `textured-rect-flip-point-sampled` entry (correct — flip is implemented now);
  that is unrelated to this regression. Leave that change in place (it should be
  committed).

## Your job
1. Find WHAT Pass 2 added that changed these three cases' hand-derived key. Read the
   `textured-rect-line16/17-low-t9x` case builders and their expected-key function in
   the runner; trace every shared symbol they read; find the Pass 2 addition that
   collides. (Hint: a shared texel array, a TLUT/palette table, an OTHER_MODES or
   tile constant, or a helper fn reused by both old and new cases.)
2. Fix it so the three `low-t` cases return to `wgpu_matches_key=True` WITHOUT
   weakening or deleting them, and WITHOUT changing render-wgpu source. The right fix
   is almost certainly to rename/scope the colliding new symbol or give the new case
   its own local data, so the old cases' key is restored. Do NOT "fix" it by editing
   the old cases' expected values to match new output — that would hide a real key
   corruption.
3. Also resolve the 2 `all-three-differ-inspect-construction` cases
   (`gen-blend-deep-two-cycle-textured-bilerp`, `gen-widerformats-ci8-triangle-two-cycle`):
   these are the integrator's flagged construction suspects. Either fix their
   construction so they classify cleanly (pass-all-match-hardware, wgpu-refused, or a
   legitimate shared-ported-bug), or if a case is fundamentally mis-built, DROP it
   with a one-line note. A case that "differs on all three" is usually malformed.
4. Sanity-check the two huge-diff shared-ported-bug cases:
   `gen-two-cycle-combined-alpha-chain` (d=320) and
   `gen-blend-deep-im-rd-striped-framebuffer` (d=38400 = whole framebuffer). A
   whole-framebuffer diff almost always means a mis-constructed case or a wrong
   seed/key, NOT a real shared divergence. Investigate; fix construction or drop with
   a note. Do NOT leave a 38400-pixel "shared-ported-bug" in the committed corpus
   unless you can show it is a genuine wgpu==RT64-vs-angrylion divergence with a real
   cause.
5. The other shared-ported-bug cases (CI/TLUT family: gen-tlut-*, gen-triangle-ci*,
   the loadblock CI8, the FORCE_BL/fog ones) are EXPECTED — they are the known #20
   CI S-plane divergence and the task-19 log-only domains. Leave them, but in your
   report state which known domain each belongs to so none is an unexplained new bug.

## Verify (REQUIRED)
- Build: FN64_RT64_DIR=$HOME/Code/no-mercy-recompiled/third_party/rt64 cargo build -p fn64-render-conformance --features parity-runner --bin fn64-render-conformance-parity-runner --offline
- Triage: FN64_RT64_DIR=$HOME/Code/no-mercy-recompiled/third_party/rt64 FN64_GENERATE=1 target/debug/fn64-render-conformance-parity-runner > triage.json (parse with python3). Full runner may STALL in wgpu/Metal device init at 0% CPU — kill and re-run, a re-run gets through; or use FN64_ONLY=<substr> for targeted runs. macOS has NO `timeout` command.
- GATE MUST PASS: FN64_RT64_DIR=... target/debug/fn64-render-conformance-parity-runner > gate.json ; python3 scripts/check_rt64_parity.py < gate.json  -> "RT64 PARITY GATE: PASS -- 33/37". The three low-t cases must be back to matching their key. No new unaccounted divergence.
- Final classification tally should show 0 fn64-defect and 0 all-three-differ (you resolved/dropped them).

## Commit
Commit on branch worktree-wm2000-playable (do NOT push). Include the runner AND the
check_rt64_parity.py change (the flip-entry removal) so the gate is self-consistent.
NOTE: `git commit -- <paths> -m "..."` mis-parses (the -- swallows -m); use
`git add <paths>` then `git commit -m "..."`. Do NOT commit README/Cargo.* .
Message: test(parity): fan-out pass 2 -- widen matrix (2-cycle/blend/tlut/lod/zmode/formats), fix low-t key regression

## Report
Write `.superpowers/sdd/i-want-to-pick-melodic-hopcroft/track-B-fanout-pass2-report.md`:
the root cause of the low-t regression (the exact colliding symbol), how you fixed it,
what you did with the 2 all-three-differ + 2 huge-diff cases, the final classification
tally, the gate result, and the commit hash. Return the commit hash + a concise summary.

## Constraints
- Serial: you are the ONLY writer in this shared tree. Do NOT create a git worktree
  (it will fail). Do NOT dispatch subagents.
- Do NOT modify render-wgpu source — this is a corpus/runner-only task.
- angrylion oracle is external/MAME — reference OUTPUT only, never link it.
