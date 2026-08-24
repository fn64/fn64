# WM2000 renders a wrestling ring in attract mode, on the all-Rust stack

Captured on `lane/wm2000-match-drive` (from `port/rt64-conveyor` 8cf9df5a, the
tree where the swap-1901 `lookup` trap is fixed), `FN64_RENDER=wgpu`,
`FN64_RECOMP=rs`. No `--features rt64`, no reference renderer.

## What these frames are

The frames described here are retained in operator scratch only because they
contain game output; this directory keeps the measurement record, not the
PNGs.

`fn64-fb-<swap>.png` is the harness dump, which reads the guest framebuffer at
a hardcoded 320x240 (`capture_framebuffer` in `packages/wm2000-boot/src/main.rs`).
WM2000 actually scans out **480x237**, so the raw dump shears every row.
`reframed-swap-<swap>.png` is the same bytes re-laid at stride 480 by
`docs/tools/wm2000-reframe.py`, recovering the top 160 true rows unsheared.
The analysis used the reframed copies; the raw copies remain only in operator
scratch as the unmodified record.

## What they show

**Swap 560 and 580: a wrestling ring, in 3D, animating.** Red ring ropes run
across the frame -- horizontally at swap 560, diagonally at swap 580 as the
camera swings around the ring. Large flat-shaded body-coloured shapes (white,
green, blue) move between the ropes. Swap 700 shows the ropes from a third
camera position.

This is **attract-mode demo play**: it appears at swap ~560, well before the
first scripted button fires at swap 1100, so no input produced it. The game is
playing itself.

## Why this is worth recording

The ring is composed and animated, not frozen. Over swaps 500-620 the run
dumped **121 frames, 121 of them distinct** -- every single frame unique. The
geometry is unmistakably an N64 wrestling ring: ropes, canvas, and moving
character bodies.

What is NOT yet right is surface detail. Bodies render as large untextured
flat-shaded polygons rather than textured wrestlers, and swap 500 (a few frames
earlier) is a washed-out near-white fade-in. So the *scene graph* reaches the
rasterizer and the *ring* draws correctly, while character surfaces do not
resolve. That is a texturing/shading gap, not a geometry or a display-list gap.

**Nonclaim.** This is attract-mode demo play, not a player-controlled match
reached through the menus. It does not by itself show the menu path reaches a
match. What it does show is that the render path can already put a recognisable
animated ring on screen, which bounds where the remaining gap can be.
