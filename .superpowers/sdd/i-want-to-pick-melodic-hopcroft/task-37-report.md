# Task 37 — live 480-wide intro capture: rightmost-column stale-RDRAM confirmed

## Verdict up front

**Candidate (b) CONFIRMED.** The rightmost pixel column (col 479 of the 480-wide
framebuffer) is a **live-only, uncovered column holding stale RDRAM** — content
carried over from a prior frame, never the current frame's fill/scene value. Cols
0–478 are covered by the per-frame fill/render; col 479 is not. This is a
**fill/clear coverage gap**, not a texel over-read (ruled out task 36) and not a
triangle right-edge rounding bug (ruled out task 35).

The fix belongs at the **fill/clear seam**: the per-frame covered extent stops at
col 478 while the framebuffer (VI_WIDTH) is 480 wide.

## Capture provenance (valid, not reference-fallback)

- Lane: all-fn64 Rust stack. Stack banner confirmed
  `recompiler : rs (fn64-cpu-runtime)` · `renderer : wgpu` (registered, **not**
  reference-fallback) · `game : linked`.
- Run: `scripts/play-wm2000.sh` with `FN64_RECOMP=rs FN64_RENDER=wgpu`,
  `FN64_FRAME_DUMP=/private/tmp/task37/frames`, bounded by
  `FN64_PUMP_CENSUS=1 WARMUP=300 PUMPS=400`. Exited 0 via the census
  window-complete path (`[pump-census] RENDERER: wgpu pumps=400`). No unbounded
  GUI loop, no source change, no commit, no worktree.
- Present surface: `resized present surface to 480x237 (game VI_WIDTH x VI active
  output lines)`.
- **142 PNGs captured, ALL 480×237.** No 320-wide / reference-fallback dumps.
  Frames: `/private/tmp/task37/frames/frame-0000-*.png`.

## The measured columns 476–479 (decisive)

A full-run scan of every frame (`/private/tmp/task37/scan_all.py`) shows the same
signature everywhere the frame has covered content:

- **col 477 ≡ col 478 in essentially every row** (mean |478−477| luminance diff
  ≈ 0–5). The covered scene reaches at least col 478.
- **col 479 diverges from the row on all 237 rows**, with |479−478| far exceeding
  the |478−477| scene baseline.

Representative frames (`/private/tmp/task37/detail.py`):

| idx | cols 0–478 (fill/scene) | col 479 | reading |
|---|---|---|---|
| 69  | uniform white `(255,255,255)` all rows | uniform `(8,8,8)` all rows | uncovered: leftover, not the fill |
| 114 | uniform white `(255,255,255)` all rows | dark red gradient `(33,0,0)…(82,33,0)` | **stale content** from an earlier reddish frame showing through |
| 120 | gray `(214,214,214)` scene | `(0,0,0)`, `(33,74,132)`, `(8,25,49)` — darker, wrong hue | not the current row's color |
| 127 | `(99,99,99)` scene | `(0,8,16)`, `(49,74,115)` — muted/lagging | not the current row's color |

The col-479 values are **neither constant-background nor scene-correlated**: they
are muted/prior-frame bytes that partially resemble content but lag the current
frame (e.g. idx 114: whole frame filled white, but col 479 still carries the
previous reddish scene). That is the exact signature of **stale RDRAM in an
uncovered column** — candidate (b). Fade sequences (idx 56–68) make it especially
clear: cols 0–478 ramp together while col 479 stays behind, i.e. it never
receives the fill.

Discriminator against candidate (a)/(rounding): if this were a texel over-read or
a right-edge rounding artifact it would be scene-*correlated* (a shifted/wrong
version of the row). It is not — col 479 is temporally stale, uncorrelated with
the current row's value. And task 35/36 already proved the CPU rasterizer skips
zero-coverage pixels (writes the resident's own byte) and the texrect sampler
addresses the rightmost texel correctly. The only remaining mechanism is an
uncovered column, which is what the live capture shows.

## Where the fill/clear fix belongs

The wgpu fill clip is faithful (`crates/fn64-render-wgpu/src/targets/fill.rs:283-299`):

```
first_x = rectangle.x0().max(scissor.first_column());
limit_x = (rectangle.x1() + 1).min(scissor.column_limit());   // exclusive
...
x1 = limit_x - 1;                                             // inclusive covered right edge
```

The covered right edge is `min(guest_rect.x1+1, scissor.column_limit()) - 1`.
Since col 478 is covered and col 479 is not, the effective covered extent is
`[0, 479)` (cols 0–478) while the framebuffer / VI scanout is **480** wide. So the
gap is that WM2000's per-frame fill/clear (and/or its scissor `column_limit`)
extends to 479, not 480 — leaving the last column at whatever RDRAM already held.

This is a fill/clear-coverage issue, not a rasterizer-math issue. The fix seam is
the fill/clear extent vs the presented framebuffer width:
- `crates/fn64-render-wgpu/src/targets/fill.rs` (the clip that resolves the covered
  right edge from rectangle + scissor), and
- the VI scanout width that presents col 479
  (`crates/fn64-render-wgpu/src/vi_scanout.rs`).

Real N64 hardware hides this last column via VI overscan / H_START (the CRT never
scans out the extreme right column), so on real hardware the stale col 479 is
never visible; fn64 presents the full 480-wide scanned rectangle, so it surfaces.
The clean fix is either to extend the per-frame clear to cover the full presented
width (guarantee col 479 is written each frame) or to not present the uncovered
overscan column — the former is the safer, hardware-faithful choice (clear the
whole framebuffer the guest owns).

**To nail the exact guest-side right coordinate** (rect x1 vs scissor
column_limit == 479 vs 480) a one-line probe on the fill/scissor decode would
print it; that is a code change (out of this read-only-run task's scope), but the
pixel evidence already localizes the gap to col 479 unambiguously.

## Artifacts
- Frames (142 × 480×237): `/private/tmp/task37/frames/frame-0000-*.png`
- Run log (banner + census): `/private/tmp/task37/run.log`
- Scan scripts: `/private/tmp/task37/scan_all.py`, `/private/tmp/task37/detail.py`,
  `/private/tmp/task37/inspect2.py`
