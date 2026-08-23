# Task 35: diagnose the "rightmost pixel is noise" defect — READ-ONLY, capture-and-diff

## Signal (from the owner)
On WM2000 (all-fn64 rs+wgpu), **the rightmost pixel (right edge) is noise** — the
owner suspects a **rounding error**. "Noise" = garbage/wrong values along the
right-hand column, not a subtly-wrong-but-plausible color.

## Not the parallel raster change (already ruled out)
The just-committed data-parallel scanline path (f864242e) is BYTE-IDENTICAL to the
scalar path (agreement test passes) and the parity gate is PASS 33/37 with it
default-on. So it neither causes nor fixes this — the defect is pre-existing. Do
NOT chase the rayon code. (You can set FN64_PARALLEL_RASTER=0 to force scalar and
confirm the noise is identical either way.)

## Leads (verify, don't assume)
1. **Right-edge subpixel rounding** — `crates/fn64-render-wgpu/src/raw_dpc/triangle_span.rs:218`:
   `x1 = ceil_ratio(max_right - Q16_ONE/8, Q16_ONE).clamp(0, clamp_width)`.
   x0 uses `min_left - 7*Q16_ONE/8`. This is the covered-range right edge from a
   subpixel coverage rule. If x1 is one too large, the rightmost column is a pixel
   the triangle doesn't actually cover — sampled from stale/garbage texture or
   framebuffer state = "noise". If one too small, the real edge is dropped. Check
   the rule against the RDP/oracle right-edge convention (memory
   [[fillrect-cycle-edge-rules]]: Fill includes lower/right edge, 1-/2-cycle
   EXCLUDES it — a texrect/triangle including the right edge when it shouldn't
   would read a column past coverage).
2. **texrect vs triangle vs fill** — which primitive shows the noise? The right-edge
   rule differs by primitive+cycle. Determine which path WM2000's noisy content uses.
   Right edge of texrects: `targets/texrect.rs` clipped extent. Right edge of fills:
   the fill-rect path.
3. **Read past the covered run** — if x1 exceeds the declared journal run
   `[x0, x1)`, the sampler reads a texel/color for an uncovered pixel: check whether
   the sample coordinate at x1-1 vs x1 steps past the texture's valid extent (TMEM
   wrap/clamp/mirror at the right tile edge) or past the framebuffer row.
4. **Scissor right edge** — S10.2 scissor derivation (there's a known
   scissor-narrower-than-rect RT64-divergence case); an off-by-one in scissor right
   clamp would leave the rightmost column unscissored.

## Method (capture-and-diff, NOT speculation — memory [[diff-oracle-before-hypothesising]])
- Reproduce in the parity corpus if possible: is there a case whose right-edge
  column differs wgpu-vs-angrylion/RT64? If not, construct a hand case (a rect/
  triangle whose right edge lands on a fractional subpixel boundary) and 3-way
  compare. The rightmost-column bytes are the evidence.
- If it only shows on live WM2000, use the frame-dump path (FN64_FRAME_DUMP if it
  exists; the task-22 agent dumped 540 PNGs) and inspect the right column of a
  scene the owner would see it on.
- Pin the EXACT wrong bytes and the exact x-index, then trace back to which rule
  (x1 rounding / scissor / sample-past-extent) produced them. Distinguish PROVEN
  (captured bytes) from INFERRED.

## Deliverable
Root cause of the rightmost-pixel noise (the exact rule + file:line + wrong bytes),
which primitive/path it affects, whether it reproduces in the parity corpus, and a
proposed fix (do NOT implement — propose, with the byte-identity/gate plan a fix
would need). If it's the known scissor-S10.2 or an edge-rule the corpus already
documents, say so.

## Constraints
- READ-ONLY. No code changes, no worktree, no commits (temp capture/diff scripts OK
  if reverted). angrylion oracle external/MAME — reference OUTPUT only.
- Report to `.superpowers/sdd/i-want-to-pick-melodic-hopcroft/task-35-report.md`.
