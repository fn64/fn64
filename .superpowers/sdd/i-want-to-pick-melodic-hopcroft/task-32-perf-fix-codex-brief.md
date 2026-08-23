# Task 32: make WM2000 hit the 30Hz budget on the all-fn64 rs+wgpu lane — OWN IT

## The goal (owner's directive)
WM2000 on `FN64_RECOMP=rs FN64_RENDER=wgpu` currently runs **~49 ms/drawn frame =
1.47x over the 33.3 ms 30Hz budget (~20.4 fps, target 30)**. Visuals are clean.
**Fix the performance so it hits (or gets as close as achievable to) 33.3 ms/drawn
frame.** You own the diagnosis AND the implementation — don't just analyze,
deliver a working, verified speedup.

## What's already established (don't re-litigate these — build on them)
- **The cost is the CPU triangle rasterizer**, not GPU/present/digests. `raster_triangle`
  (`crates/fn64-render-wgpu/src/targets/raw_triangle.rs`) is ~62-66% of the render
  field at ~483-507 ns/covered-pixel. WM2000 presents via the CPU rasterizer writing
  guest RDRAM, which VI scans out.
- **Micro-optimization is exhausted.** Measured null on: texture-plane stepping (~1%),
  hoistable setup (≤0.14%), texel read/TMEM/TLUT (~0.3%), combine (+0.08%), blend
  (below noise). The ~483 ns/px is the bare scalar per-pixel loop + the irreducible
  RDP combiner/blender math. A reported "z-buffer regression" was a THERMAL
  MEASUREMENT ARTIFACT (min-of-N transplant refuted it) — do not chase it.
- Conclusion from all that: **closing a 1.47x gap needs an ARCHITECTURAL move, not
  more micro-opt.** The prime candidate is promoting the GPU triangle pipeline.

## The prime lever: promote the GPU triangle pipeline
- `crates/fn64-render-wgpu/src/targets/triangle_pipeline.rs` (~2203 lines) already
  implements GPU rasterization with depth/blend/TMEM/coverage wired — but it's
  **diagnostic-only**: it outputs to `triangle_draw_output`, NOT guest RDRAM, and
  `present` refuses to scan it out.
- Promotion path (from the plan's Risk section): GPU draw → render target →
  **readback to guest RDRAM** at the SetColorImage extent → VI scanout. The
  architectural mismatch to solve: guest framebuffer is **RGBA16 at SetColorImage
  extent**, GPU output is **RGBA8 at RenderConfig extent** — requantization +
  resize needed. Per-frame readback latency must be worth it vs the ~43 ms CPU cost.
- **This is the hard part and the real fix.** If you can make the GPU path produce
  byte-correct (or visually-correct within the parity tolerance) guest pixels and
  it's faster, that's the win. If full byte-parity with the CPU rasterizer isn't
  achievable, get as close as possible and REPORT the exact remaining
  divergence — do not ship visibly-wrong output.

## You have full authority to
- Diagnose with your own tooling first (confirm the CPU-cost attribution on the
  current HEAD with a proper measurement — min-of-N, true baseline, watch thermal
  drift; the machine throttles on long runs so keep runs bounded and interleaved).
- Implement the GPU-pipeline promotion, OR any other architecture that measurably
  closes the gap if you find a better one (e.g. parallelizing the CPU raster across
  cores — WM2000's raster is single-threaded; a data-parallel span/tile split could
  also close 1.47x and is lower-risk than GPU readback. Your call — measure and pick).
- Change production code. This is a real implementation task, not read-only.

## Hard requirements (non-negotiable)
1. **Measure the win for real.** Before/after drawn-frame ms on the SAME scene,
   renderer-tagged (wgpu+rs), UNPROFILED mean, min-of-N to beat thermal drift.
   State the shipped figure as unprofiled-mean (×2 if you measure per-field). The
   target is 33.3 ms/drawn frame; report how close you got.
2. **Correctness gate — do NOT ship visibly-wrong output.** The RT64 parity gate
   must still PASS 33/37 (`python3 scripts/check_rt64_parity.py`; full runner may
   stall in Metal init — kill+rerun or FN64_ONLY). If you promote the GPU path,
   the guest pixels it produces must match the CPU rasterizer's output on the
   parity corpus (or you document the exact tolerated divergence and why it's
   acceptable). The wgpu lib suite must stay green.
3. **If the fix isn't fully achievable in one pass**, ship the part that works
   with its measured win + correctness, and report precisely what remains. A
   partial, honest, verified speedup beats a broken "complete" one.

## Method / environment notes
- Invoke the `fn64-perf-method` skill; read its REFERENCE.md closed-lines ledger.
- Deterministic scene: WM2000 attract pump-census (FN64_PUMP_CENSUS=1
  FN64_PUMP_CENSUS_WARMUP=300 FN64_PUMP_CENSUS_PUMPS=1200) is the shipped-frame
  measurement; the headless `texture_plane_raster_microbench`
  (raw_triangle/tests.rs) is the per-pixel A/B substrate. The windowed census
  needs a GUI — if you can't drive it, use the microbench for per-pixel deltas and
  reason to the frame figure, and say the windowed confirmation is pending.
- `scripts/play-wm2000.sh` runs the ROM (rs+wgpu). macOS has no `timeout`.
- `git commit -- <p> -m` mis-parses; use `git add` then `git commit -m`. Branch
  worktree-wm2000-playable, do NOT push. Commit incrementally as pieces verify.

## Report
`.superpowers/sdd/i-want-to-pick-melodic-hopcroft/task-32-report.md`: the approach
chosen (and why over the alternative), before/after drawn-frame ms (measured,
renderer-tagged, min-of-N), the correctness proof (parity gate + pixel match/
documented divergence + suite), commit hash(es), and what remains if not fully at
budget. Deliver the fix — this is the owner's top priority.
