# Task 6c (measure): bracket combine vs blend — the deciding rasterizer measurement, READ-ONLY

## Why this is the decision point
Elimination so far (all headless-microbench-measured, byte-identity-proven):
- texture-plane arithmetic: ~1% of per-pixel cost — CLOSED (Task 27, null+reverted)
- hoistable setup (preflight/AddressScope/snapshot): ≤0.14% — CLOSED (Task 28)
- whole `read_texel` (TMEM read + decode + TLUT): ~1.6 ns of 507 ns/px (~0.3%) — CLOSED
So **~505 of 507 ns/covered-pixel now lives in exactly two per-pixel functions**:
- `combine_one_texel` — `crates/fn64-render-wgpu/src/targets/texrect.rs:2072`
- `blend_and_write_pixel` — `crates/fn64-render-wgpu/src/targets/texrect.rs:2647`
both called per pixel from the raster loop (texrect.rs:1899 combine, :1917 blend).

This task BRACKETS the two so we know which (if either) is a real optimization
target above the ~0.5 ns/px resolution floor. Per fn64-perf-method rule 32 this is
likely a **both-halves-must-fall** case — measure each before ANY writer. NO
production changes.

## Substrate
The kept headless microbench `texture_plane_raster_microbench`
(`crates/fn64-render-wgpu/src/targets/raw_triangle/tests.rs:1181`, `#[ignore]`,
`--ignored --nocapture`), pure-CPU `raster_triangle` at a fixed covered-pixel count.
Add temporary per-RUN brackets around the combine call and the blend call
separately (NOT per-pixel — per-pixel instrumentation distorts +13%; bracket the
whole run and divide by pixel count). REVERT all temp brackets; report shares.

## Watch the traps Task 28 hit (read its report first)
`.superpowers/sdd/i-want-to-pick-melodic-hopcroft/task-6b-sample-measure-report.md`
documents two artifacts that faked a 97.9% reading: a test helper allocating per
pixel (`try_new` = 457 ns/px), and `black_box` forcing a round-trip. Do NOT
introduce allocation or forced conversions inside the timed region — time the real
production call path only.

## What to produce (measurement + verdict, NO code changes)
1. ns/px share of `combine_one_texel` and of `blend_and_write_pixel` on the fixed
   scene (≥3 reps, report mean + spread). Confirm they sum to ~the ~505 ns/px
   residual (arithmetic must close, per method).
2. Within whichever is larger, identify the hot sub-stages. For blend: coverage-alpha,
   alpha-compare, dither, the read/blend/write — which are routed as calls even when
   they're identity/no-op (the fast-path candidate)? For combine: the per-slot input
   mux + arithmetic.
3. **VERDICT, one of:**
   - **A confirmed candidate above the floor** — with a kill-evidence sketch (what to
     fast-path/restructure, expected ns/px mechanism, the microbench A/B, byte-identity
     plan: fast-path==general-path identity test + parity gate 33/37 + unchanged device
     bytes). Say which function and which sub-stage.
   - **NEAR THE FLOOR** — if the cost is diffuse across combine+blend with no
     sub-stage above ~a few % / the resolution floor (i.e. it's just the irreducible
     per-pixel combiner+blender math the N64 RDP semantics require), say so PLAINLY.
     That is the decisive finding: micro-opt cannot close the 1.47x gap and the
     remaining lever is ARCHITECTURAL (GPU raster), a user decision.

## Constraints
- READ-ONLY. Temp brackets OK only if reverted + reported. No worktree, no commits.
- Invoke fn64-perf-method; closed lines (plane-stepping, hoistable-setup, texel-read)
  are closed — don't re-measure them. Read shares not absolutes; numbers are pure-CPU.
- Do NOT fabricate a shipped-frame figure (GUI census, out of scope).

## Report
`.superpowers/sdd/i-want-to-pick-melodic-hopcroft/task-6c-report.md`: combine vs blend
ns/px shares (arithmetic closing), the hot sub-stage if any, and the VERDICT
(confirmed-candidate-with-kill-evidence-sketch OR near-the-floor). Return a concise
verdict as your final message — this determines whether we micro-opt once more or
escalate the architecture decision to the user.
