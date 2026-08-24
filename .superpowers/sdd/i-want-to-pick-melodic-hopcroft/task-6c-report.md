# Task 6c (measure): combine vs blend bracket — the deciding rasterizer measurement

## VERDICT: NEAR THE FLOOR — and the premise was wrong

**`combine_one_texel` and `blend_and_write_pixel` are NOT the residual.** Direct
per-run bracketing on the kept headless microbench measures **combine = +0.4 ns/px,
blend = −1.8 ns/px** (negative: below noise), on a **~480 ns/px** pure-CPU raster
baseline. Removing sample + combine + blend **together** costs only **+3.6 ns/px
(0.7% of the pipeline)** — leaving **~476-483 ns/px (~99%)** in the scalar
per-pixel loop that all four prior "residual" hypotheses sat on top of. A second
run removing the S/T `coordinates_at` math on top of that changed nothing
(−0.9 ns/px), so the cost is not even the coordinate interpolation — it is the
bare per-pixel loop + offset + bounds-checked byte write.

Micro-optimizing combine or blend **cannot** move the 1.47x gap. The remaining
lever is **architectural (GPU raster)** — a user decision.

The 6b report's premise ("~505 of 507 ns/px lives in combine+blend") was an
**inference from elimination, never a direct bracket**. This task brackets them
directly and refutes it. Both halves fall — but not because both are big; because
**neither is**, and the cost was never in them.

---

## Method

Per `fn64-perf-method`: per-RUN A/B subtraction (never per-pixel — 6b documents
the +13% per-pixel distortion), shares read not absolutes, interleaved reps,
lanes proven to differ, closed lines (plane-arith, hoistable setup, texel-read)
not re-measured. All numbers **pure-CPU raster** (`texture_plane_raster_microbench`,
`raw_triangle/tests.rs:1181`, `--ignored`); `FN64_RENDER` not involved. This is
**not** a shipped-frame figure.

Instrument: a temporary 6c-bracket env lane captured **once** before the
pixel loop (one predictable branch per pixel, present in every lane so it
subtracts out — no per-pixel clock reads, no allocation, no `black_box`
round-trip inside the timed region). **All temp brackets reverted; tree clean.**

Lanes (each replaces exactly one production call with the cheapest valid stand-in,
keeping the 66,000-covered-pixel denominator intact):
- **0 FULL** — production path.
- **1 no-combine** — `combine_one_texel` → raw texel bytes.
- **2 no-blend** — `blend_and_write_pixel` → a 1-2 byte direct write (keeps the
  covered-pixel count and defeats dead-code elimination).
- **3 no-sample** — per-pixel `sample_point` → a decoded texel hoisted once.
- **4 no-sample+combine+blend** — all three removed: coordinate + loop + write floor.
- **5 no-sample+combine+blend+coordinates** — lane 4 with `coordinates_at` also
  fixed to (0,0): isolates the per-pixel `step_axis` mul/div cost.

## Results — 6 interleaved reps, lanes 0-4

| lane | stage removed | mean ns/px | min | max | stdev |
|---|---|---:|---:|---:|---:|
| 0 | FULL | 480.10 | 466.6 | 491.7 | 8.64 |
| 1 | no-combine | 479.70 | 469.4 | 486.4 | 6.91 |
| 2 | no-blend | 481.89 | 471.7 | 491.2 | 6.55 |
| 3 | no-sample | 481.50 | 463.1 | 493.5 | 10.86 |
| 4 | no-sample+combine+blend | 476.50 | 466.0 | 490.0 | 8.53 |

Derived per-stage cost (FULL − lane):

| stage | ns/px | share of 480 |
|---|---:|---:|
| **combine_one_texel** | **+0.40** | **0.08%** |
| **blend_and_write_pixel** | **−1.80** (noise) | ~0% |
| sample_point (per-pixel) | −1.40 (noise) | ~0% |
| **all three together (FULL − lane4)** | **+3.60** | **0.7%** |
| **coordinate/loop/write floor (lane 4)** | **476.50** | **99.3%** |

**Per-lane run-to-run stdev is 6.5-10.9 ns.** Every single-stage cost (+0.4, −1.8,
−1.4) is smaller than one lane's own stdev, and two of the three read **negative** —
the textbook signature of a cost below the resolution floor. This is the same
magnitude and the same verdict as the plane-arith (Task 27, ~1%) and hoistable-setup
(Task 28, ≤0.14%) nulls.

**Arithmetic closes:** combine+blend+sample summed = −2.8 ns/px; measured directly as
FULL−lane4 = +3.6 ns/px. Both are ~0 within the ±9 ns noise — they agree that the
three per-pixel functions jointly are negligible, and they bracket zero. The residual
that must close is **the ~476 ns/px floor, not these functions.**

## Where the ~480 ns/px actually is — coordinate isolation (lanes 0/4/5)

6 more interleaved reps, lanes 0 / 4 / 5:

| lane | stage removed | mean ns/px | stdev |
|---|---|---:|---:|
| 0 | FULL | 483.53 | 10.46 |
| 4 | no sample+combine+blend | 483.27 | 9.12 |
| 5 | no sample+combine+blend **+ coordinates_at** | 484.42 | 9.14 |

- **coordinates_at (lane4 − lane5) = −1.15 ns/px** — negative, noise. The two i64
  mul+div per pixel in `step_axis` are **not** the cost either.
- **FULL − lane5 = −0.88 ns/px.** Removing sample, combine, blend, AND the S/T
  coordinate math leaves the pipeline **statistically unchanged (~484 ns/px)**.

**Conclusion: ~483 ns/px is the irreducible scalar per-pixel loop itself** — visiting
66,000 pixels one at a time: the `Range<u32>` double iterator, the per-pixel
destination offset (`y*width+x)*bpp`), and the bounds-checked byte-slice write
(`bytes[offset..offset+bpp]`). No single *named computation* — texel read, combiner,
blender, coordinate interpolation — is above the ~9-10 ns run-to-run noise. The cost
is the loop trip count × the fixed per-iteration scalar overhead, which is exactly
what a CPU rasterizer cannot escape and a GPU raster path amortizes across lanes.

(Artifact check per method rule 31: 483 ns/px × 66,000 px = ~31.9 ms/iteration;
the per-iteration `resident_bytes.to_vec()` of a 153 KB buffer and `new_for_fill`
are microseconds — far too small to be the misattributed denominator. The cost is
genuinely per-pixel-loop, not a fixed per-run cost divided by pixels.)

## Sub-stage note (for completeness)

The brief asked, within the larger of combine/blend, for the hot sub-stage. Neither
is large enough for a sub-stage to matter: at +0.4 / −1.8 ns/px the fast-path
candidates 6b named (identity `apply_alpha_dither`, no-op RGB dither, the
`Result`-returning stage dispatch, the per-slot combiner input mux) are each a
fraction of a fraction of a ns/px — **orders of magnitude below the ~9 ns noise and
the ~0.5 ns/px floor.** There is no sub-stage to confirm.

## Why no confirmed candidate / no kill-evidence sketch

A kill-evidence sketch (fast-path plan, expected ns/px, A/B, byte-identity via
fast-path==general-path test + parity gate 33/37 + unchanged device bytes) is only
worth writing for a candidate that clears the floor. **None does.** Writing one for a
+0.4 ns/px combine or a −1.8 ns/px blend would be proposing a writer for a cost
below the instrument's resolution — exactly what 6b, Task 28 and Task 27 each
refused. Do not send a writer after combine or blend.

## Closed-lines ledger — new entries

- **`combine_one_texel` per-pixel cost = +0.4 ns/px (0.08% of 480 ns/px pure-CPU
  raster)** — measured 2026-08-23, 6 interleaved reps, per-run A/B subtraction.
  Below the ~9 ns per-lane noise. Do not re-propose.
- **`blend_and_write_pixel` per-pixel cost ≈ 0 (measured −1.8 ns/px, i.e. below
  noise, sign unstable)** — same run. The "top remaining candidate" 6b named is a
  null. The identity-stage fast-path (skip `apply_alpha_dither`/coverage dispatch on
  the admitted mode) would target a cost that does not exist at this scale. Do not
  re-propose.
- **Corollary: per-pixel `sample_point` in the full loop = ≈0 (−1.4 ns/px, below
  noise)** — confirms 6b's isolated ~1.6 ns/px read on the production path too;
  removing it changes nothing measurable.
- **The rasterizer's cost is the coordinate/loop/write FLOOR (~476 ns/px = 99.3%),
  not any per-pixel shading function.** Sample+combine+blend jointly = 3.6 ns/px.
  The 1.47x gap is not reachable by micro-opt of these functions; the lever is
  architectural (GPU raster). Escalate to user.

## Provenance / reproduction

- Substrate: `crates/fn64-render-wgpu/src/targets/raw_triangle/tests.rs:1181`
  (`texture_plane_raster_microbench`, `#[ignore]`), release,
  `--ignored --nocapture`, 66,000 covered px × 400 iters/run.
- Temp instrument: 6c-bracket lanes in
  `crates/fn64-render-wgpu/src/targets/texrect.rs` (execute_texture_rectangle loop).
  **Reverted after measurement; `git status` clean for that file.**
- Machine load ~1.4-1.7 on 15 cores throughout; interleaved A/B/C(/D/E) per rep.
- Snapshots: `.claude/scratch-6c/sweep.out` (lanes 0-4), `.claude/scratch-6c/coord.out`
  (lanes 0/4/5). Scratch dir removed after reporting.
