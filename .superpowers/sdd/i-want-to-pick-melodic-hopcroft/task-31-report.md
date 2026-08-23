# Task 31: the fcd48b7c "depth-disabled regression" was a MEASUREMENT ARTIFACT

## Headline

Task 30 reported that `fcd48b7c` left +10.06 ns/px (+2.04%) of per-pixel depth
bookkeeping on WM2000's depth-disabled raster path. **A properly-controlled
3-point microbench transplant refutes that: the pre-regression parent and the
regressed HEAD are indistinguishable (mean parent-vs-HEAD delta = -0.35 ns/px,
sign split 6/6 across 12 triplets).** The regression does not reproduce. It was
a thermal/contention measurement artifact of task-30's parent-vs-commit A/B, not
a real cost — LLVM already elides the dead `depth == None` per-pixel work (the
match is loop-invariant `None`; the linear pixel index is recomputed as the byte
`offset` anyway, so it was never redundant).

**There was nothing for a fast path to recover.** Per the kill-evidence gate
(commit only if ns/px recovers), no production change ships. This report
retroactively corrects task-30.

## What was measured (the decisive experiment)

Three genuinely distinct release binaries, differing ONLY in the per-pixel
depth code in `raster_triangle`'s covered-pixel loop, each built by swapping the
one source file into the same target dir (the `#[ignore]`
`texture_plane_raster_microbench`, 66,000 covered px x 400 iters, pure-CPU
raster, unprofiled):

- **A = parent (pre-fcd48b7c shape):** HEAD's `raw_triangle.rs` with the
  per-pixel depth machinery physically stripped from the loop (no `pixel` index,
  no `match (depth.as_ref(), fragment_depth)`, no `if !passes_depth`, no depth
  commit) — verified absent by grep. Keeps the `depth: None` API so the bench
  compiles unchanged. This is the true ~492 baseline task-30 named.
- **B = HEAD (regressed):** current `worktree-wm2000-playable` HEAD, carrying
  the unconditional per-pixel depth match at `raw_triangle.rs:681-682`.
- **C = candidate fix:** a depth-free fast path (see "The fix that was written").

Protocol: **ABCCBA within each triplet** (so all three share thermal state),
**min of the two reads per side** (least-perturbed), **12 triplets**, machine
otherwise quiet. Per-triplet deltas cancel slow thermal drift; the session's
absolute ns/px drifted 478 -> 536 as the box heated, which is exactly why
task-30's non-interleaved parent-vs-commit split produced a spurious +10.

### Results (ns/px)

| | A (parent) | B (HEAD) | C (fix) |
|---|---:|---:|---:|
| mean (12 triplets) | 500.23 | 499.89 | 501.36 |
| min-of-N | 483.43 | 486.62 | 493.46 |

| delta (mean) | value | meaning |
|---|---:|---|
| **B - A** | **-0.35** | the claimed +10 regression: **does not reproduce** |
| C - A | +1.13 | fix vs parent: null |
| C - B | +1.47 | fix vs HEAD: null (no recovery to make) |

Per-triplet `B - A` ranges -17.7 .. +16.3 ns/px, 6 positive / 6 negative — pure
noise around zero. The ±16 ns/px per-triplet spread swamps the 10 ns/px effect
task-30 claimed, confirming that effect was within the noise floor.

Earlier, cruder A/Bs in the same session (against HEAD only, not the parent)
told the same story once order-balanced: a first ABBA run gave HEAD-vs-fix delta
-1.23 (parity); a 30-rep run gave mean delta -0.37. The signal was never there.

## The fix that was written (and then reverted)

To have a candidate C to measure, a depth-free fast path was implemented in
`raster_triangle`: branch once on `depth` presence OUTSIDE the row/column loops,
and run a depth-free loop body carrying NO Z pixel index, NO `Option` match, and
NO compare/update checks for the `None` case, with the depth-present path kept
byte-for-byte semantically identical. It was verified structurally correct
(the depth-free arm has zero depth machinery) and byte-identical (the agreement
test below passes).

Two implementation shapes were tried:
- A **closure** sharing the pixel body across both loops: measurably *regressed*
  (+7 ns/px vs HEAD, ABBA-balanced) — the call / `Result` / `Option` boundary in
  the hot per-pixel path defeated optimization.
- An **inline `macro_rules!`** expanding the shared body into each loop (no call
  boundary, each loop specialized): the C measured above, at parity.

Because C never beat A or B, and the compiler already handles the `None` case,
the production change is complexity for zero measurable benefit. **Reverted:
`raw_triangle.rs` is byte-identical to HEAD** (`git diff HEAD~1 HEAD --
raw_triangle.rs` is empty).

## What shipped

Commit **`2908e419`** — `test(render-wgpu): assert raster_triangle depth path is
a no-op when depth disabled` — the guard test ONLY (111 insertions in
`raw_triangle/tests.rs`, no production change). It rasterizes the microbench's
shaded+textured (0x0e) triangle twice — once with `depth: None`, once with a
`Some(RawTriangleDepth)` whose `Z_CMP` and `Z_UPD` are both clear (cells seeded
non-zero) — and asserts (1) byte-identical framebuffers and (2) the seeded cells
are untouched. This documents and pins the z-buffer feature's invariant (the
per-pixel depth decision is a no-op when disabled), guarding the real feature
against a future edit silently changing a non-z draw's output. It compiles and
passes against the reverted HEAD code.

## Correctness / verification

- Full `fn64-render-wgpu` lib suite: **4887 passed, 0 failed** (release).
- Agreement test passes against reverted (HEAD) production code.
- No z-buffer parity gate run was needed: the depth-ENABLED path is unchanged
  (byte-identical to HEAD), and the depth-DISABLED path is likewise byte-identical
  to HEAD (production reverted). Byte-identity is therefore trivial, not measured.

## Verdict

- ns/px recovery: **N/A — no reproducible regression exists** to recover
  (B - A = -0.35 ns/px, within noise).
- Byte-identity: **holds trivially** — production `raw_triangle.rs` is unchanged
  from HEAD on both paths.
- **Committed:** `2908e419` (guard test only). **No production perf change** —
  the target regression was a measurement artifact.

## Method note (for the ledger)

This is a textbook case for `fn64-perf-method` rules 4 (measure only on a quiet
machine), 5 (interleave A/B — and here A/B/C), and the closure-tolerance
discipline. Task-30's parent-vs-commit A/B was interleaved but ran across a
warming machine without a min-of-N or within-triplet control; the +10 ns/px it
saw was thermal drift wearing the name of a code change. The refutation cost a
3-point transplant and 12 ABCCBA triplets.
