# Task 23 report: characterizing the rt64-hle-defect `gen-blend-aa-sloped-edge`

READ-ONLY investigation. No code changed, no worktree created.

## Verdict

**Yes — this is a real, backportable RT64 HLE defect.** RT64 writes 2 partial-coverage
edge pixels on the sloped hypotenuse that bit-accurate hardware (angrylion) and wgpu both
leave as background. The root cause is that RT64's HLE rasterizer/blender **does not emulate
sub-pixel edge coverage**: it makes a binary in/out decision at 1 sample per pixel and treats
the blender's coverage input as full coverage. It belongs on the RT64 issue-review card.

## The case (from the parity runner)

Builder: `gen_blend_aa_edge_rect(HALF_ALPHA_RED, 65536/2)` in
`crates/fn64-render-conformance/src/bin/fn64-render-conformance-parity-runner.rs`
(push site ~line 4484, builder ~line 4373, `sloped_triangle_words` ~line 4361).

- Geometry: one sloped triangle, `dXHdy = 0.5` (Q16 `65536/2`). Anchored at
  `TRI_LEFT=2`, `TRI_TOP=0`, `TRI_BOTTOM=3`. The hypotenuse column shifts right by
  0.5 px per scanline, so the right-hand edge crosses pixel boundaries mid-column ->
  the edge pixels have fractional coverage.
- Background: whole framebuffer pre-filled with `STALE = 0xffff` (white RGBA5551),
  then the fill's final pipe-sync popped (`one_fill(...); words.pop()`).
- SetOtherModes: `blend_deep_other_modes(cycle=0, p1=0,a1=0,m1=0,b1=1, p2=0,a2=0,m2=0,b2=0,
  force_bl=true, im_rd=true, aa_en=true)`.
  - One-cycle. Blend cycle-1 mux: **P1=CombinedColor, A1=CombinedAlpha, M1=MemoryColor,
    B1=1 (= FRAMEBUFFER_ALPHA = the stored coverage of the memory pixel).**
  - `FORCE_BL=1`, `IM_RD=1` (read-modify-write against framebuffer), `AA_EN=1`.
  - Encoded high/low: `0xef00_00f0` / low with `b1<<16 | 1<<14 | 1<<6 | 1<<3`.
- Combine: `SET_COMBINE_PRIMITIVE` (`0xfcff_ffff / 0xfffd_f6fb`) -> combined color = primitive.
- Primitive: `HALF_ALPHA_RED = 0xff00_0080` (R=255,G=0,B=0, alpha=128).

Intent as documented in the case: "antialiased partial-coverage edge ... hypotenuse
column coverage varies by row."

## Three-way result (built + run)

Build: `FN64_RT64_DIR=$HOME/Code/no-mercy-recompiled/third_party/rt64 cargo build -p
fn64-render-conformance --features parity-runner --bin
fn64-render-conformance-parity-runner --offline` -> exit 0.

Run: `FN64_RT64_DIR=... FN64_GENERATE=1 FN64_ONLY=blend-aa <binary>` (did NOT stall).

`gen-blend-aa-sloped-edge` -> classification **`rt64-hle-defect`**:
- `wgpu_vs_angrylion_diff_pixels = 0`  (wgpu == angrylion exactly, buffer-wide)
- `rt64_vs_angrylion_diff_pixels = 2`
- First differing pixel (RT64 vs angrylion): **pixel 323, x=3, y=1** —
  angrylion = `0xffff` (untouched STALE background), RT64 = `0xfbdf`
  (the half-red primitive blended over the white background).
- 2 differing pixels total; both are the fractional-coverage hypotenuse edge pixels
  (x=3 on the two interior scanlines y=1 and y=2, given `dXHdy=0.5`).

Sibling `gen-blend-aa-coverage-driven-edge` (same geometry, B=FramebufferCoverage
selector) classified `wgpu-refused`: wgpu explicitly refuses framebuffer-alpha-dependent
blending ("this crate does not yet implement framebuffer-alpha-dependent blending"), so
that sibling gives no wgpu==angrylion signal. It is NOT the rt64-hle-defect case; the
sloped-edge case is.

## The precise RT64 mechanism (from RT64 HLE source)

Two cooperating simplifications in RT64, both in files under `$FN64_RT64_DIR/src`:

1. **No sub-pixel edge coverage in the rasterizer.**
   `src/shaders/RasterPS.hlsl` (coverage estimation, ~lines 215-255):
   ```
   float resultCvg = (8.0f / cvgRange) * (otherMode.cvgXAlpha() ? combinerColor.a : 1.0f);
   const float CoverageThreshold = 1.0f / cvgRange;
   if (resultCvg < CoverageThreshold) return false;   // discard
   ```
   Coverage is `8/cvgRange` (full) for every pixel the GPU rasterizes, scaled only by
   the combiner alpha when `CVG_X_ALPHA` is set (it is NOT set here). There is no
   fractional edge-coverage term derived from the triangle edge equations. The GPU
   rasterizes at 1 sample/pixel with the pixel-center rule, so an edge pixel is either
   fully in (full coverage, written) or fully out (discarded). N64 hardware instead
   computes per-pixel fractional coverage from sub-pixel edge crossings; on this sloped
   edge that fraction is small enough that hardware's coverage-blend leaves the two edge
   pixels visually at background, whereas RT64 writes them at full strength.

2. **The blender's coverage input is hardcoded to full.**
   `src/shared/rt64_blender.h`, `fromInputB` (~line 351):
   ```
   case B_FRAMEBUFFER_ALPHA:
       return 1.0f;   // "Coverage is not emulated. We intentionally return full
                      //  coverage whenever it's requested."
   ```
   B1=1 in this case is exactly `B_FRAMEBUFFER_ALPHA` (enum value 1, confirmed at
   rt64_blender.h line 34). On N64 hardware with AA_EN + IM_RD, that B input is the
   stored memory coverage and is what attenuates an edge pixel toward the background.
   RT64 returns a literal `1.0f`, so even the blend equation cannot recover the missing
   edge attenuation. The source comment states the omission is intentional.

Net: both the coverage computation and the coverage-consuming blend input are stubbed to
"full", so RT64 has no path to reproduce a fractional AA edge on a sloped triangle.

## Is wgpu genuinely correct here?

- wgpu matches angrylion with **0 differing pixels across the whole 320x240 buffer** —
  not an accidental single-pixel coincidence; it reproduces the exact hardware coverage
  behavior on the edge.
- Caveat (per the brief and `[[angrylion-mame-license-blocks-oracle]]`): angrylion is
  MAME-licensed and is clean-room-EXCLUDED as the SOLE authority. There is no independent
  hand-derived expectation for these specific edge pixels in this case; the confirmation
  rests on angrylion alone. What angrylion DOES give us admissibly is a strong signal:
  wgpu agreeing with bit-accurate hardware AGAINST RT64 means RT64 is the outlier and has
  a real bug. To make wgpu's correctness admissible without angrylion, add a hand-derived
  key for the two edge pixels (compute the N64 fractional coverage for `dXHdy=0.5` at x=3,
  y=1/2 and the resulting coverage-attenuated blend), or corroborate against a second
  non-MAME bit-accurate reference.

## Backportability / issue-review card

- **Real RT64 defect:** yes. Mechanism is well-localized and reproducible.
- **Backportable to RT64 upstream:** yes in principle, but non-trivial — it requires
  implementing sub-pixel edge coverage (e.g. MSAA/analytic coverage in RasterPS plus a
  real coverage value feeding `B_FRAMEBUFFER_ALPHA`) rather than the current intentional
  full-coverage stub. The source comment ("Coverage is not emulated. We intentionally
  return full coverage") shows this is a deliberate HLE simplification, so a fix is a
  feature-level change, not a one-line correction.
- **Belongs on `[[rt64-issue-review-card]]`:** yes. Recommended card contents:
  - Symptom: sloped/antialiased partial-coverage edges are written at full strength;
    hardware leaves low-coverage edge pixels at background. 2-pixel divergence in the
    minimal repro; larger visible AA-fringe differences on real geometry.
  - Root cause: `RasterPS.hlsl` binary coverage (no edge fraction) + `rt64_blender.h`
    `fromInputB` returning `1.0f` for `B_FRAMEBUFFER_ALPHA` (intentional stub).
  - Repro: `gen-blend-aa-sloped-edge` (`FN64_ONLY=blend-aa`), pixel (3,1)
    angrylion `0xffff` vs RT64 `0xfbdf`.
  - Class: RT64 HLE defect (mirror image of shared-ported-bug cases); wgpu is correct.

## Note

This is the only `rt64-hle-defect` case surfaced by fan-out Pass 2. It is distinct from the
texrect-coverage class in the progress notes (where the oracle scope itself was contested);
here angrylion is authoritative and wgpu matches it cleanly.
