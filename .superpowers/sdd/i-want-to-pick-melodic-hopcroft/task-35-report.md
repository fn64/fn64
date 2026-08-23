# Task 35 — rightmost-pixel-noise diagnosis (READ-ONLY, capture-and-diff)

## Verdict up front

- **The brief's prime lead is REFUTED, twice over.** The right-edge subpixel
  rounding at `crates/fn64-render-wgpu/src/raw_dpc/triangle_span.rs:218`
  (`x1 = ceil_ratio(max_right - Q16_ONE/8, Q16_ONE)`) is **provably exact** for
  every eighth-pixel right-edge position (numeric proof below): declared `x1-1`
  always equals the true rightmost pixel that has a subpixel sample strictly
  inside the triangle. There is no off-by-one. And even if it *were* one too
  large, it could not produce noise, because the CPU triangle rasterizer
  **skips** zero-coverage pixels (`targets/raw_triangle.rs:44-58`, the
  `attribute_sample` -> `None` -> `continue` arm at `:640-648`): a declared
  edge pixel with no covered subsample is written back with **the resident's
  own current byte**, i.e. real current framebuffer content, never stale/garbage.

- **The corpus does NOT reproduce a rightmost-pixel defect.** I built and ran
  the 3-way parity runner (wgpu vs RT64 vs angrylion-seeded key) with
  `FN64_RT64_DIR=$HOME/Code/no-mercy-recompiled/third_party/rt64`. Every
  right-edge-convention case — `single-pixel`, `last-column-last-row`,
  `one-cycle-fill-band` (which pins the EXCLUSIVE right edge: final column keeps
  the seed), and all `textured-rect-*` and `flat/shade/textured-triangle` cases
  — is `wgpu_matches_key = true` / `verdict = identical`. PROVEN totals: 35/39
  identical, 3 `differs` are unrelated (RT64/angrylion divergences the runner
  already documents), 1 real wgpu key mismatch (`two-cycle-textured`, pixel
  x=0 — a combiner defect, NOT a right-edge one). No CPU path emits a wrong
  right-edge column on the corpus.

- **Most likely real mechanism (INFERRED, not yet captured live):** the on-screen
  content for WM2000 is written by the **CPU texrect path** (`targets/texrect.rs`)
  and the CPU fill/triangle paths into RDRAM — the GPU `draw_admitted_triangles`
  path is unpresented (memory `wm2000-perf-gpu-draw-is-unpresented`; confirmed at
  `production.rs:2357`). The texrect path is the **only** live path that writes
  every pixel of its `[first_column, column_limit)` extent unconditionally (no
  per-pixel coverage skip). Its own history records exactly the
  "rightmost-pixel-is-garbage" symptom class and a fix for it: a TMEM first-row
  parity bug where "each rectangle row's **last pixel** read a byte the load never
  wrote" (`targets/texrect.rs:1846-1850`, pinned by `tmem/read.rs`'s
  `wm2000_texrect_*` tests, byte `0x04c`). That specific bug is fixed. Any
  residual rightmost-column noise is in this family: a right-edge texel sampled
  past what the tile's load actually wrote.

## PROVEN vs INFERRED

PROVEN (captured/derived bytes):
1. Triangle `x1` rule is exact — numeric check over 0..2000 eighth-pixel right
   edges, 0 mismatches (`/tmp/x1check.py`, reproduced below).
2. Corpus 3-way run is clean on every right-edge case.
3. Triangle raster skips zero-coverage edge pixels (source-read of the loop).
4. Fill path uses the correct INCLUSIVE right edge (`width = x1-x0+1`,
   `targets/fill.rs:187`); corpus proves it (`single-pixel`,
   `last-column-last-row` identical).
5. Presentation (`fn64-shell/src/framebuffer.rs::rgba5551_to_rgba8888`) is a
   straight per-pixel copy at `src_stride = VI_WIDTH` (480 for WM2000); the last
   column is copied exactly (`full_width_surface_presents_the_whole_line...`
   test). No resampling/rounding — presentation is not the noise source.

INFERRED (not captured on a live current frame this session):
- That the residual live noise is texrect right-edge TMEM over-read. I could not
  capture a current live frame: the committed frame dumps (`docs/frames/wm2000-*`)
  predate the current tree (raw dumps are 320x240; reframed 480-wide ones show
  only the top 160 rows, right edge black). A live capture needs the interactive
  shell (ROM + `RECOMP_RS_HOST_LOOKUP` are present on this machine), which mutates
  this worktree's `crates/fn64-shell/rs/recompiled` symlink and `Cargo.toml` —
  disallowed by READ-ONLY. See "Recommended next capture".

## The x1 numeric proof

```
Q=1<<16; ceil_ratio(n,d) = -((-n)//d)
for R (right edge in px) over 0..250 in 1/8 steps:
    max_right = round(R*Q); x1 = ceil_ratio(max_right - Q/8, Q)
    true rightmost covered pixel xmax = ceil(R - 1/8) - 1   # x s.t. x+1/8 < R
    assert x1-1 == xmax          # holds for ALL R: 0 mismatches
```
The `-1/8` is the FIRST subpixel column; a pixel is left as soon as its first
sample (x+1/8) passes the right edge — the exclusive one-/two-cycle right edge
(memory `fillrect-cycle-edge-rules`), implemented correctly.

## Why the corpus can't catch the live defect

Corpus targets are 320x240 and no case drives a **textured** rectangle whose
right edge lands on the framebuffer's last column while sampling a tile at the
edge of what its load wrote. Textured cases sample a small 4x2 image well inside
the tile. The bug lives at the intersection of (a) texrect, (b) the right screen
column, and (c) a tile whose valid texels end at that column — a shape the
corpus does not construct. Documented blind spot: `parity-runner.rs:1127`
("A defect there is invisible to the whole corpus").

## Proposed fix (do NOT implement — proposal only)

No fix belongs in `triangle_span.rs`; that lead is a dead end. Two testable
proposals, priority order:

1. **Reproduce, then fix at the texrect tile-addressing seam.** Add a parity
   case: a one-cycle `G_TEXRECT` whose right edge is `x = WIDTH-1`, sampling a
   tile whose loaded S extent ends exactly at the last column — one variant with
   `mask_s == 0` (clamp), a sibling with `mask_s != 0` (wrap). If the wrap
   variant's last column reads the tile's left edge (a color discontinuity vs
   the RT64/angrylion key), that is the bug; the fix is to confirm
   `s_at(width-1)` never rounds up into the next texel — audit `step_axis`'s
   truncating division (`targets/texrect.rs:372-379`) and the
   `PLANE_TO_TEXEL`/`>>5` texel quantization (`tmem/sample.rs`) at the boundary
   against RT64's float coordinate. Byte-identity plan: keep all 35 currently-
   identical corpus cases identical; turn the new right-edge case to
   `identical` (wgpu == key); gate = parity-runner PASS count must not regress.

2. **If capture shows stale RDRAM, not a wrong texel:** column 479 of WM2000's
   480-wide color image is never covered by any admitted primitive and retains
   uninitialized RDRAM that VI scans out. Fix belongs at the fill/clear seam
   (does the frame's initial `G_FILLRECT` cover column 479?), not the sampler.
   Byte-identity plan: a full-target fill case at width 480 with the last column
   asserted against the fill's inclusive-edge key.

## Recommended next capture (the missing PROVEN step)

From a NON-worktree checkout (so the play script's symlink/Cargo edits do not
touch this READ-ONLY worktree):

```
FN64_FRAME_TRIP=/tmp/wm-trip.txt FN64_FRAME_DUMP=/tmp/wm-frames \
  ROM=$HOME/Code/aki-recomp/games/NWXE/wm2000.z64 \
  RECOMP_RS_HOST_LOOKUP=$HOME/Code/recomps/wm2000/packages/wm2000-boot/src/host_lookup.rs \
  ./scripts/play-wm2000.sh
```

The tripwire auto-exits after `capacity` frames; `FN64_FRAME_DUMP` writes each as
a 480-wide PNG. Inspect columns 476..479 across many rows of a content frame
(attract ring, swap ~560). Repeated wrong-but-plausible color -> proposal 1
(texrect wrap over-read). Varied, scene-uncorrelated bytes -> proposal 2 (stale
RDRAM in an uncovered column). One dump distinguishes them and pins the exact
wrong bytes + x-index.

## Files inspected (read-only)

- `crates/fn64-render-wgpu/src/raw_dpc/triangle_span.rs` (x1 rule — exact)
- `crates/fn64-render-wgpu/src/targets/raw_triangle.rs` (skips zero-coverage)
- `crates/fn64-render-wgpu/src/targets/texrect.rs` (writes full extent; parity fix history)
- `crates/fn64-render-wgpu/src/targets/fill.rs` (inclusive right edge — correct)
- `crates/fn64-render-wgpu/src/tmem/read.rs`, `tmem/sample.rs` (clamp/mask/mirror addressing)
- `crates/fn64-shell/src/main.rs`, `framebuffer.rs` (VI present — straight copy, 480 stride)
- `crates/fn64-render-conformance/src/bin/fn64-render-conformance-parity-runner.rs` (built + run)

Temp files in /tmp only (`/tmp/x1check.py`, `/tmp/pngcol.py`, `/tmp/parity_out.txt`);
no worktree changes, no commits.
