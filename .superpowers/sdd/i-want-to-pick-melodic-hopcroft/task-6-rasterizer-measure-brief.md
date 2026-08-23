# Task 6 (measure phase): profile the CPU rasterizer hot path — READ-ONLY, measure first

## Goal
WM2000 on the all-fn64 stack renders cleanly but at **49.1 ms/drawn frame = 1.47x
over the 33.3 ms 30Hz budget** (~20.4 fps). Task 22 attributed **88.9% of the
per-field overage to gfx_lle_rdp**, and within it **87.9% to the raw-DPC execute /
rasterization stage** over ~55,486 submissions. The CPU triangle rasterizer is the
wall. This task MEASURES where the rasterizer's time actually goes and returns a
RANKED optimization plan with kill-evidence design — it makes NO code changes.

## MANDATORY method (this project has burned hours on wrong perf numbers)
1. **Invoke the `fn64-perf-method` skill FIRST** (`.claude/skills/fn64-perf-method/SKILL.md`)
   and **read its `REFERENCE.md`** — especially the **closed-lines ledger** (optimizations
   already tried and REJECTED — do NOT re-propose any of them) and the environment
   contracts + byte-identity gate. Cite which closed lines you checked.
2. **State the renderer on every number.** All figures must be `FN64_RENDER=wgpu`
   `FN64_RECOMP=rs` (the all-fn64 stack). A graphics number without its renderer is
   not a result. Reference-lane numbers are meaningless here.
3. **Unprofiled mean × 2 = the shipped drawn-frame figure** (30Hz title, both fields).
   Never build a shipped figure from profiled/p50 numbers — profiling inflates,
   the distribution is bimodal.
4. **Do NOT trust leaf CPU profiles for guest coroutines** (memory
   [[sample-cannot-see-guest-coroutines]] — threads read idle at 85% CPU). Use
   PHASE COUNTERS / session-phase census, not `sample`/leaf attribution.
5. Measure on a DETERMINISTIC scene so before/after is comparable. Find the
   deterministic lever (a fixed attract/demo warmup+pumps window via
   FN64_PUMP_CENSUS_WARMUP/PUMPS reproducibly, or a captured-replay path). If none
   exists cleanly, say so and propose the smallest deterministic substrate.

## What to produce (measurement + plan, NO code changes)
1. **Within-rasterizer attribution.** The 43.24 ms render field is 87.9% raw-DPC
   execute. Break THAT down: inside `crates/fn64-render-wgpu/src/targets/raw_triangle.rs`
   (770 lines) and `crates/fn64-render-wgpu/src/tmem/sample.rs` (1304 lines), where
   do the cycles go? Candidates from the plan/memory: per-pixel `sample_point` (TMEM
   addressing + format decode + TLUT lookup per textured pixel), texture-coordinate
   stepping, coverage/subsample handling, SHA-256 content digests (~20% per
   [[wm2000-wgpu-perf-attribution]]), per-submission overhead. Get NUMBERS
   (ns/pixel, or phase shares, or call counts × cost) — not guesses. Use whatever
   instrumentation the perf-method skill sanctions; add temporary counters ONLY if
   read-only-safe and removed before reporting (or note them as a needed probe).
2. **Rank the cost centers** by measured share of the render field.
3. **For the top 2-3, a kill-evidence-ready optimization sketch:** what to change,
   the expected mechanism of speedup, the deterministic before/after measurement
   that would prove it (ns/pixel on the same scene), and the byte-identity check
   (rasterizer output must stay byte-identical — parity gate + the deterministic
   scene's pixels). Note anything that would risk correctness.
4. **Cross-check against the closed-lines ledger** — if a candidate is already
   closed, drop it and say why it was closed.

## Constraints
- READ-ONLY / measurement. No production code changes, no git worktree, no commits.
  (Temporary local instrumentation is OK only if reverted and the report says so.)
- Bounded runs only.
- Every number carries its renderer + whether it's profiled or unprofiled.

## Report
`.superpowers/sdd/i-want-to-pick-melodic-hopcroft/task-6-measure-report.md`: the
within-rasterizer attribution (ranked, with numbers + renderer + profiled/unprofiled),
the deterministic scene used, the top optimization candidates with kill-evidence
design + byte-identity plan, and which closed-ledger lines you cleared. Return a
concise ranked summary + the single highest-value candidate as your final message.
