# Task 24: LOD/mipmap path assessment — DEAD CODE, not WM2000-blocking

VERDICT: LOD path is DEAD CODE (inert port). Fix is "wire-up-existing + moderate
new work" (two tiers). NOT WM2000-blocking — any-ROM breadth only.

(Report authored by the read-only Explore agent, which lacked a Write tool; saved
by the orchestrator.)

## DEAD vs LIVE
`compute_lod` / `LodSelection` / `LodTileIndices` / `hlsl_clamp_i32` have ZERO
live referrers. Only mentions outside `texture_lod.rs` are doc-comment cross-refs
in `rt64_texture_sampler.rs` (lines 91, 121, 337) and two parity-runner comments
stating outright "wgpu never calls compute_lod" and "compute_lod unwired"
(parity-runner lines 5563, 5591). Module is declared `mod texture_lod;`
(lib.rs:411) but nothing calls it. Confirmed inert — same
[[rt64-ported-modules-are-inert]] pattern.

`compute_lod` itself is COMPLETE and correct: full literal RT64 port of
`computeLOD` (TextureSampler.hlsli:27-72), pinned SHA, ~19 unit tests (both flags,
boundaries, mutation sweep, IEEE edge cases). Computes tile_index0/tile_index1 +
lod_fraction from texture derivatives (ddx/ddy UV), res_lod_scale, primLOD, tile
count, OtherMode texLOD/texDetail. Not a stub. The CPU function is not the problem.

## WHERE IT WOULD WIRE (touch points)
wgpu hardcodes `lod_fraction = 0.0` in 3 CombinerInputs constructions:
production.rs:5786, :14059, :15470. The combiner already fully supports
LOD_FRACTION as an input on both CPU (combiner.rs:600 ColorInput::LodFraction,
:637 AlphaInput::LodFraction) and WGSL (combiner.rs:1938/1943 read
inputs.lod_fraction) — the CONSUMER exists; only the PRODUCER is missing/zeroed.

Two distinct gaps the fan-out cases hit:
1. **lod_fraction VALUE**: wgpu passes 0.0; non-mip HW/RT64 want 1.0 when
   texture_lod_en OFF, and computeLOD's result when ON.
   (gen-lod-fraction-combiner-disabled/-enabled, gen-two-cycle-lod-fraction-gap.)
2. **MIP TILE SELECTION**: wgpu samples tile 0 only; RT64 uses
   tile_index0/tile_index1 to pick/blend mip levels. Sampler side is
   tmem/sample.rs (sample_point / sample_committed_point) + rt64_texture_sampler.rs
   (sample_texture_level_blend :561, clamp_wrap_mirror_address :488) — ALSO
   ported-but-unwired.

## FIX SIZE
- **TIER A (small, wire-up)**: the value-only gap. Compute lod_fraction (call
  compute_lod, or for texture_lod_en-OFF just supply 1.0 instead of 0.0) and
  thread it into the 3 CombinerInputs sites. Fixes gen-lod-fraction-combiner-disabled
  (control, wants 1.0) outright; the enabled cases also need the wgpu draw path to
  supply ddx/ddy UV derivatives + tile descriptors to compute_lod — that data flow
  does NOT exist yet (the non-trivial part).
- **TIER B (moderate-to-large, real feature)**: actual mip tile selection/blending
  in the sampler (two resident tiles -> pick tile_index0/tile_index1 -> blend by
  lod_fraction). Requires wiring the inert rt64_texture_sampler.rs helpers +
  tmem/sample.rs into the draw path.

Honest estimate: NOT a pure 3-line wire-up. Tier A's disabled control (supply 1.0)
is genuinely small. Enabled/derivative cases need a derivative + tile-descriptor
data path into the combiner that doesn't exist. Tier B is a real feature.

## WM2000 RELEVANCE
NOT WM2000-blocking. WM2000's dominant program is wm2000_shade_fog_triangle
(parity-runner:4265), SET_COMBINE_WM2000_SHADE_FOG = (0xfc45fea3, 0xf00ff83f) — a
shade-driven, UNTEXTURED two-cycle fog triangle. No SetTile mip chain, no
texture_lod_en, no LOD_FRACTION combiner input. All WM2000 evidence docs concern
texrect/TLUT/fillrect state, never LOD. LOD fan-out cases are any-ROM breadth
(RDP completeness), not WM2000's path. CONFIRMED.

Related: [[rt64-ported-modules-are-inert]], [[rdp-untested-surface-map]].
