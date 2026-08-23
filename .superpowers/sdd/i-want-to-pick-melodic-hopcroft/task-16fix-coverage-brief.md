# Task 16fix: coverage color-on-cvg drop (CLR_ON_CVG + CVG_DST_WRAP)

## The gap (proven by fan-out Pass 1)
Case `gen-coverage-color-on-cvg-one-cycle` — wgpu DROPS a write that hardware
(angrylion) and RT64 make. When SetOtherModes sets **CLR_ON_CVG** (write color
only on coverage overflow) together with **CVG_DST_WRAP** (coverage destination =
wrap), wgpu's coverage-narrowing path declines to write the pixel while
angrylion + RT64 write it. This is the single `fn64-defect` from Pass 1's triage
(1 fn64-defect in the classification tally).

Confirm the exact case: run the runner and find the row whose classification is
`fn64-defect` (or the coverage case that diverges from angrylion). Read its
`intent` and command words in
`crates/fn64-render-conformance/src/bin/fn64-render-conformance-parity-runner.rs`
(coverage slice builders).

## Background
CLR_ON_CVG (SetOtherModes bit): color is written only when coverage overflows
(carries). CVG_DST (2 bits): clamp / wrap / zap / save — the coverage
destination mode. The combination CLR_ON_CVG + CVG_DST=wrap defines whether an
antialiased edge pixel's color reaches the framebuffer. wgpu's coverage path
(`crates/fn64-render-wgpu/src/targets/triangle_pipeline.rs` coverage narrowing,
and `crates/fn64-render-wgpu/src/coverage.rs` / `coverage/`) currently only
admits a narrow combination (per plan Task 9: passes for
`alpha_coverage_select=false && force_blend=true`) and drops others.

## What to do
1. Build the runner, `FN64_GENERATE=1`, identify the exact coverage case that
   diverges (wgpu drops the write; angrylion/RT64 make it). Capture the diff:
   `wgpu_vs_angrylion_diff_pixels` and first-diff pixel.
2. Root-cause: trace the coverage-narrowing decision in triangle_pipeline.rs /
   coverage.rs for CLR_ON_CVG + CVG_DST_WRAP. Find where wgpu decides NOT to write.
3. Fix so wgpu makes the write hardware makes. The reference for CORRECT behavior
   = angrylion (bit-accurate) + RT64. Match the hardware coverage-overflow rule
   for CLR_ON_CVG with CVG_DST=wrap.
4. Do NOT broaden beyond what the case proves — fix the specific
   CLR_ON_CVG+CVG_DST_WRAP drop, mutation-tested (reverting re-drops the write).

## Verify (REQUIRED — build and run)
Build:
```
FN64_RT64_DIR=$HOME/Code/no-mercy-recompiled/third_party/rt64 cargo build -p fn64-render-conformance --features parity-runner --bin fn64-render-conformance-parity-runner --offline
```
Triage:
```
FN64_RT64_DIR=$HOME/Code/no-mercy-recompiled/third_party/rt64 FN64_GENERATE=1 target/debug/fn64-render-conformance-parity-runner > cov.json
```
The coverage case must go from `fn64-defect` (wgpu drops) to
`wgpu_vs_angrylion_diff_pixels: 0`. If it can't reach 0, report the residual with
diff count + first-diff pixel — do NOT suppress.
Gate (must PASS):
```
FN64_RT64_DIR=$HOME/Code/no-mercy-recompiled/third_party/rt64 target/debug/fn64-render-conformance-parity-runner > gate.json
python3 scripts/check_rt64_parity.py < gate.json
```
Add a wgpu-crate unit test pinning the CLR_ON_CVG+CVG_DST_WRAP write decision so a
revert fails it.

## Constraints
- You are in an ISOLATED worktree — work here only.
- Surgical: fix the specific coverage decision; do not restructure the coverage subsystem.
- angrylion oracle is external/MAME — reference OUTPUT only. Binary:
  /Users/jer/Code/angrylion-oracle/oracle (runner calls it, fail-open).
- Commit on your worktree branch (do NOT push):
  `fix(render-wgpu): write color on coverage overflow for CLR_ON_CVG + CVG_DST_WRAP`
- Report to `.superpowers/sdd/i-want-to-pick-melodic-hopcroft/task-16fix-report.md`: the exact
  case, root cause, the fix, before/after (defect -> 0 diff), gate result, unit test,
  commit hash. Return the commit hash + a one-line summary.
