# Task 33 (measure): profile the `plan` phase — the top NON-rasterizer perf opportunity, READ-ONLY

## Why this one
Task 22's armed phase census (WM2000 attract, rs+wgpu) decomposed the graphics
raw-DPC dispatch over 55,486 submissions:
- `execute` (rasterizer): ~88% — owned by Task 32 (Codex), DON'T touch it.
- **`plan`: 11.0% (1,962.2 ms, 0.035 ms/submission)** — the single biggest
  non-execute cost, and completely orthogonal to the rasterizer.
- commit 1.1%, finalize 0.1%, vi_present flat — all sub-floor, ignore.

So the `plan` phase is the one real perf opportunity outside the rasterizer. This
task MEASURES where its 0.035 ms/submission goes and whether any of it is
reducible above the resolution floor — NO code changes.

## Where it lives
- `PlanCollector` — `crates/fn64-render-wgpu/src/production.rs:789` (struct),
  `:1006` (impl), `:1071` (`ExactRawDpcPlanVisitor` impl — the per-submission
  visit path), `:2318` (`plan_raw_dpc`). The plan phase builds the per-submission
  plan (resource accesses, texrect/triangle command collection, fill spans) before
  execute runs.
- 55,486 submissions × 0.035 ms = ~1,962 ms over the census. Per-submission work
  is small but multiplied by submission count.

## What to produce (measurement + ranked plan, NO code changes)
1. Attribute the plan phase's per-submission cost: what does `PlanCollector` do per
   submission that's expensive or repeated — Vec allocations/growth, ContentDigest
   hashing (check if the plan path hashes; memory notes digests were a cost),
   `to_vec`/clone of command buffers, the ExactRawDpcPlanVisitor traversal, resource-
   access bookkeeping? Get NUMBERS (per-submission ns, or allocation counts × cost),
   not guesses.
2. Rank the sub-costs. Identify which are per-submission-invariant (hoistable/
   poolable across the 55,486 submissions) vs genuinely per-submission.
3. For the top reducible sub-cost: a kill-evidence-ready sketch — what to change
   (e.g. pre-allocated/pooled buffers, lazy digest, avoid a clone), expected
   mechanism, the measurement that proves it (armed phase census plan-share
   before/after, or a targeted microbench of the plan path), and the byte-identity/
   correctness plan (plan output must be identical — parity gate + validate_effects
   still pass; digests are load-bearing, never remove, only reduce/defer).
4. If the plan cost is diffuse/irreducible above the floor, say so plainly — that's
   valid (means the 11% isn't easily recoverable and the gap is all rasterizer).

## Constraints
- READ-ONLY / measurement. No production changes, no worktree, no commits (temp
  local instrumentation OK only if reverted + reported).
- Invoke `fn64-perf-method`; read its REFERENCE.md closed-lines ledger. Numbers
  renderer-tagged (wgpu+rs), unprofiled/shares per the rules; thermal drift is real
  on this machine — min-of-N / bounded interleaved runs.
- Do NOT touch execute/raster (Task 32's area) — stay in the plan phase.
- Digests are load-bearing for validate_effects + publication identity — a proposal
  that removes them is wrong; only lazy/dedup/defer.

## Report
`.superpowers/sdd/i-want-to-pick-melodic-hopcroft/task-33-report.md`: per-submission
plan attribution (ranked, with numbers), the top reducible sub-cost with kill-
evidence sketch, or a plain "plan phase is diffuse/near-floor" verdict. Return a
concise ranked summary + the single highest-value candidate.
