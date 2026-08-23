# Task 6 (fix) — step texture S/T/W planes incrementally: REVERTED, NULL RESULT

**Date:** 2026-08-23
**Lane:** all-fn64 stack — `FN64_RECOMP=rs` + `FN64_RENDER=wgpu`. The perf lever
used here is a **headless, deterministic CPU microbenchmark of the pure
`raster_triangle` function** (renderer-agnostic — it is pure CPU raster over an
in-memory `TmemByteSource`, no wgpu device, no window), which is cleaner
kill-evidence than the windowed pump census (isolates the per-pixel change from
GPU/compositor/window noise) and was runnable entirely in-sandbox.
**Method:** `fn64-perf-method` skill + `REFERENCE.md` read. Candidate confirmed
NOT in the closed-lines ledger.

## Verdict

**Commit `9ffa4e69`** (kept tests/benches only; production texplane change reverted).

**REVERTED — null result.** The change is real, correct, and byte-identical, but
its measured effect on ns-per-textured-pixel is **below measurement resolution
on every lane**. Per the brief's gate ("a cleaner-looking loop that doesn't move
the number gets reverted") and memory `[[perf-measure-before-dispatching]]`, the
production change to `raw_triangle.rs` was reverted. **No production change was
committed.** Commit `9ffa4e69` contains ONLY the kept diagnostic tests/benches
(reusable kill-evidence infrastructure), per the coordinator's decision.

## The change (what was tried, then reverted)

In `raw_triangle.rs::raster_triangle`, carried a `tex_values: Option<[i64; 3]>`
run-state mirroring the existing `shade_values`, stepping the three S/T/W planes
with `attribute_plane_step` (one add) on a continued run and falling back to the
full `attribute_plane` (i128 mul + `div_euclid`) on run-start — exactly the
shade path's structure, reusing the same already-computed `continues_run`
predicate. Bit-exact by construction (the same 200,000-case-proven step identity
the shade path relies on). This is precisely Candidate A of `task-6-measure-report.md`.

## Byte-identity (correctness gate) — all PASS while the change was in place

- **Step-identity unit test** (`stepping_a_plane_across_a_continued_run_matches_the_full_formula`):
  PASS. Walks a 16-pixel run per (slope, start, y) over both signs and asserts
  `attribute_plane_step` == full `attribute_plane` at every step. **Proven able
  to fail**: mutating the step to add `plane.de` instead of `plane.dx` made it
  FAIL (perf-method rule 6a).
- **Full `fn64-render-wgpu` lib suite:** 4886 passed, 0 failed, 3 ignored. The
  `targets::raw_triangle::tests` pixel-output tests (textured, shade-interpolated)
  exercise the actual raster loop and were byte-identical.
- **RT64 parity gate** (`python3 scripts/check_rt64_parity.py` on a full runner
  run): **PASS — 33/37 rt64-authoritative cases byte-identical to the RT64 C++
  oracle** on real Metal (Apple M5 Pro), exit 0. The four non-parity cases are
  the documented expected divergences (scissor RT64_DEFECT, yuv16 capability
  gap, negative-w broken fixture, two-cycle capability gap).

So the change was fully byte-identical — it is a pure perf question, and the
answer is that there is no measurable perf.

## Perf A/B — the null result

### Whole-raster microbench (`texture_plane_raster_microbench`, `#[ignore]`)

`execute_raw_triangle` over one large 0x0e shaded+textured+perspective triangle,
66,000 covered pixels × 400 iters, 4 interleaved reps each side (renderer-agnostic
pure-CPU ns/covered-pixel):

| side | ns/covered-pixel (4 reps) | mean |
|---|---|---|
| BASELINE (full `attribute_plane` every px) | 480.1, 489.3, 491.0, 496.4 | **~489** |
| WITH CHANGE (stepped) | 487.6, 497.7, 501.2, 501.8 | **~497** |

The two distributions **overlap**; there is **no measurable improvement** (the
with-change mean is if anything marginally higher, well within the ~±10-15 ns
run-to-run noise). NOTE: these absolutes include constant per-iteration setup
(registry/candidate/device-pack), which is identical across A/B and does not bias
the delta, but does dilute the signal — hence the isolation bench below.

### Isolation bench (`texture_plane_step_vs_full_isolation`, `#[ignore]`) — the ceiling

Times ONLY the three plane computations, 120M pixels, 3 reps:

| | full 3-plane ns/px | stepped ns/px | delta ns/px |
|---|---|---|---|
| rep 1 | 1.189 | 0.673 | 0.516 |
| rep 2 | 0.905 | 0.672 | 0.233 |
| rep 3 | 0.907 | 0.673 | 0.234 |

(The two paths were asserted bit-for-bit equal in the same test.)

**The entire 3-plane arithmetic is <~1.2 ns/px, and the achievable saving is only
~0.23-0.52 ns per textured pixel** — the theoretical ceiling of this
optimization. Against a ~490 ns/px raster (this bench) or the play lane's real
94-102 ns/px, the plane arithmetic is **~1% of the per-pixel cost**. The i128
mul+div is the most expensive scalar op in the loop, but it is dwarfed by
`sample_point`/TMEM read + `combine_one_texel` + `blend_and_write_pixel`, which
are the true per-pixel majority (the measure report ranked TMEM read #2, plane
interp #3). A ~0.2-0.5 ns/px saving is below the noise floor of every available
instrument.

## Shipped drawn-frame figure (unchanged)

Because the change is byte-identical and moves no measurable time, the shipped
drawn-frame figure is unchanged from the Task 6 measure report's baseline
(rs+wgpu, unprofiled mean × 2): **~49.07 ms/drawn-frame (~20.4 fps) vs the
33.333 ms 30 Hz budget (1.47×)**. This optimization does not close any of that
gap. The remaining wall is the per-pixel sampler + combiner + blender, not the
plane interpolation.

## Where the perf effort should go next (the useful part of this null)

The kill-evidence redirects the whole rasterizer effort. The per-pixel wall is
**NOT** the plane arithmetic (~1%, proven above). It is `sample_point` /
TMEM-read + `combine_one_texel` + `blend_and_write_pixel`, which together are
~490 ns/px in this bench (and are the play lane's real 94-102 ns/px). The
measure report's Candidate B (hoist `preflight` / `AddressScope::of` /
`snapshot()` out of the per-pixel `read_texel` — loop-invariant setup recomputed
per pixel) targets that majority and is the higher-value next candidate. **The
deterministic headless raster microbench added by this task is the substrate to
measure it** — same lever, swap the candidate.

## Files

- **`crates/fn64-render-wgpu/src/targets/raw_triangle.rs`** — the production
  texplane-stepping change was **REVERTED** (`git checkout`); the file is back at
  HEAD. No production behaviour change committed.
- **`crates/fn64-render-wgpu/src/raw_dpc/triangle_span/tests.rs`** — **KEPT +
  COMMITTED**: the step-identity test (documents & guards the proven
  `attribute_plane_step == attribute_plane` equivalence) and the
  `texture_plane_step_vs_full_isolation` `#[ignore]` bench (the ceiling number).
- **`crates/fn64-render-wgpu/src/targets/raw_triangle/tests.rs`** — **KEPT +
  COMMITTED**: `texture_plane_raster_microbench` (`#[ignore]`), the first clean
  deterministic HEADLESS ns/covered-pixel A/B substrate for `raster_triangle` —
  reusable kill-evidence infrastructure for the next rasterizer candidate.

**Commit (kept tests/benches only):** `9ffa4e69` on branch worktree-wm2000-playable,
not pushed. The production texplane change is NOT in it.

## Provenance / honesty notes

- Every perf number is renderer-agnostic pure-CPU raster (stated), unprofiled,
  ≥3 reps. The shipped drawn-frame figure is unprofiled-mean × 2, carried from
  the measure report (not re-measured, because the change is byte-identical and
  the GUI census lane was not run).
- The candidate was NOT in the closed-lines ledger; this report closes it: **the
  texture-plane step is bit-exact but its win is below measurement resolution
  because plane arithmetic is ~1% of the per-pixel cost.** Do not re-propose
  without a new, finer instrument or evidence that plane interp grew.
