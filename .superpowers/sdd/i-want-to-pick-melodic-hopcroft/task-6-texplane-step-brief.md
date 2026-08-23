# Task 6 (fix): step the texture S/T/W planes incrementally (mirror the shade path)

## The optimization (measured highest-value, Task 6 measure report)
`raster_triangle` is ~62-66% of WM2000's render field at 94-102 ns/covered-pixel.
The single highest-value candidate: the 3 texture planes call the FULL
`attribute_plane` (an **i128 multiply + `div_euclid`**) on EVERY textured pixel,
while the 4 shade planes already step incrementally with a single add on a
continued run. Mirror the shade path for the texture planes: replace
3×(i128 mul+div) per pixel with 3×add per pixel on a continued run.

## Exact site + template (already located — confirm, don't re-derive)
File: `crates/fn64-render-wgpu/src/targets/raw_triangle.rs`.
- **Template (shade, already incremental)** at ~lines 529-538:
  ```
  let continues_run = previous_sample.is_some_and(|(prev_y, prev_x)| { ... });
  shade_values = match (shade, shade_values, continues_run) {
      (Some(planes), Some(values), true)  => attribute_plane_step(planes[c], values[c]),  // add
      (Some(planes), _, _)                => attribute_plane(planes[c], delta_y_eighth, delta_x), // full, run-start
      ...
  };
  ```
- **Target (texture, currently full every pixel)** at ~lines 599-605:
  ```
  let stw: [i64; 3] = core::array::from_fn(|component| {
      triangle_span::attribute_plane(planes[component], delta_y_eighth, delta_x)
  });
  ```
  These feed `texture_coordinates_s10_5(stw, perspective)`.

## What to do
1. Carry incremental texture-plane run-state alongside the shade run-state (a
   `tex_values: Option<[i64; 3]>` mirroring `shade_values`), stepping with
   `attribute_plane_step` when `continues_run` (the SAME predicate — already
   computed) and falling back to full `attribute_plane` on run-start / non-textured.
   Reuse the shade path's exact structure so the two stay symmetric.
2. The identity `attribute_plane_step` == `attribute_plane` on a continued run is
   the SAME one the shade path relies on (proven, `triangle_span.rs:404-426`,
   200,000 cases). Do NOT weaken it; if `continues_run` is false the full call must
   still run. Perspective divide (`texture_coordinates_s10_5`) is unchanged —
   only the S/T/W plane VALUES are stepped, then fed to the same divide.
3. Keep it surgical — this is a per-pixel-loop change in one function, no
   restructuring.

## KILL-EVIDENCE (REQUIRED — this is the whole point; revert if it fails)
Follow the `fn64-perf-method` skill (.claude/skills/fn64-perf-method/SKILL.md) and
its REFERENCE.md. This candidate is NOT in the closed-lines ledger (the measure
agent cleared it) — but re-confirm.
- **Before/after on the SAME deterministic scene** (WM2000 attract pump-census,
  the lever the measure agent used). Report ns-per-TEXTURED-pixel (raster wall ÷
  combiner-census textured-pixel count — apply the perf-method rule for the pixel
  denominator) AND the shipped drawn-frame figure = **UNPROFILED mean × 2**, both
  tagged `FN64_RENDER=wgpu FN64_RECOMP=rs`. ≥2 reps each side, interleaved A/B.
- **The change MUST measurably reduce ns/pixel.** If it does not, REVERT it and
  report the null result — a cleaner-looking loop that doesn't move the number is
  reverted (plan Task 6 rule, memory [[perf-measure-before-dispatching]]).
- **Byte-identity (correctness gate):** rasterizer output must stay byte-identical.
  Prove via (a) the frozen tuple / 200k-case identity tests still green, (b) parity
  gate PASS 33/37 (`python3 scripts/check_rt64_parity.py` on a full runner run —
  full runner may stall in Metal init, kill+rerun or FN64_ONLY), and (c) the
  deterministic scene's dumped frames unchanged vs baseline (a frame tripwire, e.g.
  120/120 frames identical). Any pixel change = a correctness regression, not a win.
- Add/keep a unit test that a mutated (broken) step is caught.

## Constraints
- Serial: you are the ONLY writer in the shared tree /Users/jer/Code/fn64/.claude/worktrees/wm2000-playable (your cwd). Do NOT create a git worktree. Do NOT dispatch subagents.
- Only touch raw_triangle.rs (+ its tests, + triangle_span.rs only if a helper is genuinely missing). No other renderer changes.
- Every number carries renderer + profiled/unprofiled.

## Commit (ONLY if kill-evidence shows a real improvement AND byte-identity holds)
`git add <paths>` then `git commit -m` (the `-- -m` form mis-parses). Branch
worktree-wm2000-playable, do NOT push. Message:
  perf(render-wgpu): step texture S/T/W planes incrementally in raster_triangle (byte-identical, <before>->​<after> ns/px)
If the result is null/negative, do NOT commit — revert and report.

## Report
`.superpowers/sdd/i-want-to-pick-melodic-hopcroft/task-6-texplane-report.md`: before/after
ns/textured-pixel + shipped ms/drawn-frame (renderer-tagged, unprofiled, ≥2 reps),
byte-identity proof (which gates/tripwire), commit hash or "reverted — null result",
and the new drawn-frame figure vs the 33.3ms budget. Return a concise before/after +
verdict as your final message.
