# The renderer is not the path to 60 fps

**Read this before scoping, estimating, or reporting any renderer performance
work.** It is short because the conclusion is one number.

## VERIFIED: an infinitely fast renderer still misses the budget

`docs/plans/perf-method.md:3234-3260`, re-read and confirmed here. Set graphics
to **exactly zero** and take everything else in `executor_ns`:

| row | rep1 (ms) | rep2 (ms) |
|---|---:|---:|
| translated guest code | 9.528 | 9.462 |
| **mirror boundary** | **8.848** | **8.797** |
| invalidate writes | 2.018 | 1.961 |
| PARKED | 0.574 | 0.563 |
| other OS-call work | 0.248 | 0.242 |
| devtime | 0.246 | 0.243 |
| guards + residual | 0.138 | 0.135 |
| **HOST-SIDE TOTAL** | **21.554** | **21.361** |
| **vs 16.667 ms budget** | **1.29x** | **1.28x** |

Verified two independent ways per rep -- by subtraction
(`executor_ns − gfx_ns − audio_lle_ns`) and by summing named rows -- agreeing
to 0.046 ms.

**So a renderer speedup, however large, cannot by itself reach 60 fps.** The
remaining 1.29x is runtime apparatus plus the CPU lane. Any RT64 or
`fn64-render-wgpu` perf claim must say this explicitly, or a reader will take a
renderer win as a path to 60 that it is not.

Note the source doc corrects itself in place on exactly this point: its author
first wrote that the non-graphics rows "sum to well under the budget",
asserting a sum without computing it, and left the error visible when the real
figure inverted the conclusion. Worth emulating.

## WM2000 renders at 30 Hz, so the budget is 33.333 ms

A drawn frame gets **two** field budgets. Measuring WM2000 against 16.667 ms
overstates the gap by 2x. (`.claude/skills/fn64-perf-method/SKILL.md`)

## A benchmark trap that silently invalidates renderer numbers

The benchmark script does **not** export `FN64_RENDER`, so it defaults to the
**software rasterizer** (`.claude/skills/fn64-perf-method/SKILL.md:29-38`).
Any renderer benchmark must state which backend it actually exercised. This
project has already shipped three "defects" that were measurement artifacts;
this is the same shape.

## Where the time actually goes (measured, block lane, RT64)

`docs/plans/rt64-on-the-block-lane.md:483-494`, render field 27.68 ms,
perturbation-corrected:

| component | ms | share |
|---|---:|---:|
| graphics | 14.91 | 53.9% |
| -- rasterization | 8.30 | |
| -- **RSP interpretation** | **5.09** | |
| -- staging memcpy | 1.45 | |
| recompiled guest CPU | 8.23 | 29.7% |
| invalidate writes | 1.68 | |
| audio LLE | 0.98 | |
| mirror boundary | 0.16 | |

**RSP interpretation is 5.09 ms sitting inside the graphics bucket and is NOT
rasterization.** A full RSP->Rust recompiler exists at
`crates/fn64-audio/src/rsp/recomp/`, but it is out-of-tree artifact generation
rather than the live path. Wiring it in is a renderer-adjacent win that is not
an RT64 change -- and it is larger than most rasterization fixes on offer.

## Two numbers that are frequently miscited

`docs/plans/rt64-on-the-block-lane.md:175-178, 296-306`: the **11.9x** and
**1.28x** figures are **RT64-vs-reference RENDERER** speedups, not Rust-vs-C
CPU results. 11.9x was measured on the function lane (`wm2000-boot`, carrying
the `rt64` feature), not the block lane, and both are disclaimed as
non-comparable because 1.28x predates the `abc7871` nested-writer fix
(44.13 -> 22.51). Do not carry either as a CPU or general claim.

## Precedent: apparatus wins are cheap, renderer wins are not

A **one-line** mirror fix moved the mirror boundary from 25.9% of `executor_ns`
to 0.16 ms -- worth -20% shipped frame time, **14.3 -> 29.0 fps**
(`docs/plans/NEXT.md:9-16`). Set that against the effort every renderer
micro-optimisation in this project has cost, and prefer measuring the apparatus
first.

## Method caveats the repo enforces on itself

- **No automated perf regression gate.** `RELEASE-GATE.md` gates determinism and
  byte-identity only, so a perf regression ships silently.
- **The profiler once inflated its own subject by 26.4%.** Report profiler
  overhead alongside any number.
- **A predicted +0.029 ms landed at +1.62 ms -- 56x off**
  (`perf-method.md:113-140, 2298-2299`). Predictions here are not evidence.
