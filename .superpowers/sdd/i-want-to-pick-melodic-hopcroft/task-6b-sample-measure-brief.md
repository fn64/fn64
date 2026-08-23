# Task 6b (measure): profile within sample_point/read_texel — Candidate B, READ-ONLY

## Why
Task 27 proved the texture-plane arithmetic is only ~1% of the ~490 ns/covered-pixel
raster cost (null result, reverted). The real per-pixel wall is **`sample_point` /
TMEM read + combine + blend**. Task 27's agent named the highest-value next
candidate: **hoist per-pixel-invariant work out of the per-pixel texel read** —
`preflight` / `AddressScope` / committed-`snapshot` setup that is recomputed every
pixel but is constant across a triangle's covered run.

This task MEASURES where sample_point's per-pixel time goes and confirms (or refutes)
that hoistable invariants are a real, measurable share — BEFORE any writer touches it.
NO production code changes.

## Substrate (already exists — use it)
Task 27 committed (9ffa4e69) a reusable deterministic headless microbench:
`texture_plane_raster_microbench` in
`crates/fn64-render-wgpu/src/targets/raw_triangle/tests.rs:1181` (`#[ignore]`, run
with `--ignored --nocapture`). It times pure-CPU `raster_triangle` over a fixed
textured triangle at a known covered-pixel count — the clean ns/px A/B substrate.
Reuse/extend it; do NOT rebuild the windowed census (GUI-blocked, noisy).

## The sample path (already located)
- `sample_point` — `crates/fn64-render-wgpu/src/tmem/sample.rs:406` (per-pixel texel
  fetch over `TmemByteSource`); `sample_committed_point` at :385.
- Look for per-pixel work that is invariant across a triangle's run: preflight /
  `AddressScope` construction, committed-`snapshot` binding/validation (note the
  per-pixel `debug_assert!(... snapshot() == texels[0].snapshot())` at :444 —
  is that in release? is snapshot recomputed per pixel?), format-decode setup,
  TLUT base resolution, tile-descriptor decode.

## What to produce (measurement + ranked plan, NO code changes)
1. Using the microbench (extend it if needed for finer timing — a temporary
   sub-bracket is fine if reverted), attribute sample_point's per-pixel cost:
   how much is (a) the actual TMEM byte read + format decode + TLUT lookup
   (irreducible per-pixel), vs (b) SETUP that is constant across the covered run
   and therefore HOISTABLE (preflight/AddressScope/snapshot/tile-descriptor).
   Get NUMBERS (ns/px shares).
2. Confirm or refute Candidate B: is the hoistable setup a MEASURABLE share of the
   ~490 ns/px (i.e. worth an optimization above the ~0.5 ns/px resolution floor
   Task 27 established)? If it's <~1% like the plane arith was, say so and CLOSE it —
   don't send a writer after another red herring.
3. Also rank combine + blend (`blend_and_write_pixel` was joint-first in an old
   profile) as alternative candidates if sample_point setup turns out small.
4. For the top confirmed candidate: a kill-evidence-ready sketch (what to hoist/change,
   expected ns/px mechanism, the microbench A/B that proves it, byte-identity plan:
   identity tests + parity gate + the microbench's own pixel output unchanged).

## Constraints
- READ-ONLY / measurement. No production changes, no worktree, no commits (temporary
  local bracketing OK only if reverted and reported).
- Invoke the `fn64-perf-method` skill; re-check the closed-lines ledger (plane-stepping
  is now closed — don't re-propose). Per-pixel instrumentation distorts (+13% per
  memory) — bracket per-run/per-triangle and divide, read shares not absolutes.
- Numbers are pure-CPU raster (state that); the shipped-frame figure is a separate
  windowed-census step (GUI-blocked) — don't fabricate it.

## Report
`.superpowers/sdd/i-want-to-pick-melodic-hopcroft/task-6b-sample-measure-report.md`:
sample_point per-pixel attribution (hoistable-vs-irreducible, ns/px), Candidate B
confirmed-or-closed, the top candidate with kill-evidence sketch, closed-ledger lines
cleared. Return a concise ranked summary + the single highest-value confirmed candidate
(or "all remaining candidates <resolution — rasterizer near its floor") as your final message.
