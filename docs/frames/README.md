# Captured frames

## `wm2000-swap240-true-geometry-480x237.png`

WM2000 swap 240, guest framebuffer read from RDRAM at the VI's real geometry.
From a clean run on the all-Rust stack (`fn64-cpu-runtime` plus
`fn64-render-wgpu`, no `--features rt64`) with every guard live: 2,149 VI
swaps, exit 0, no panics.

## A retracted frame, and the mistake behind it

An earlier capture, `wm2000-allguards-swap240.png`, was committed at `272bf781`
with the claim that the lower field was "striped at a one-pixel period" and
that this was a real defect in what reaches guest memory. **That claim was
wrong, and the frame is removed rather than kept as evidence.**

WM2000 renders into a 480x237 RGBA5551 framebuffer. The capture function in the
WM2000 harness hardcodes 320x240. Reading a 480-stride buffer at a 320 stride
returns the first two thirds of each row and shears every row by 160 pixels.
The bytes in RDRAM were coherent the whole time; the reading of them was not.

Four independent measurements established this, recorded in
[the stride measurement](swap240-stripe-measurement.md):

- The pattern is period 3 at density 1/3. Interlace predicts period 2 at
  density 1/2, so interlace never fit the shape.
- Reinterpreting the same bytes at candidate strides gives vertical coherence
  0.536 at 320 (chance) against 0.967 at 480.
- Probing the live ROM found 39,543 rectangles, every one with `img_width=480`.
- Probing the VI at swap time reported `vi_width=480`, `vi_output_height=237`.

Interlace was also ruled out by register rather than by shape: WM2000's VI
STATUS is `0x00013202`, whose serrate bit is 0, so the ROM is progressive.

The lesson worth keeping: a capture path can manufacture a convincing defect.
Read the geometry from the VI registers rather than assuming 320x240, and treat
a striking visual artifact as a claim to verify, not a finding to report.

## `wm2000-menu-swap1250-*.png` — B3's "duplicated menu text", retracted

Blocker B3 in [`../RT64-WM2000-GAMEPLAY-GAP.md`](../RT64-WM2000-GAMEPLAY-GAP.md)
recorded that menu text was "horizontally duplicated (the same string tiled
~2.5x across the width)". **It is the same capture bug as the striping, and it
is not a renderer defect.**

Both PNGs here are the *same bytes* at the *same swap* of the *same run*
(Start pressed at swap 1100 per the gap doc's §3.2 script), written by the
same harness on the same pass:

- `wm2000-menu-swap1250-misread-320x240.png` — the harness's default dump
  path. The WWF logo appears twice and "Rumble Pak supported. / Insert a
  Rumble Pak now." is tiled across the width. This is the reported artifact.
- `wm2000-menu-swap1250-true-geometry-480x237.png` — the identical RDRAM
  region read at the geometry the VI actually reports. One logo, one copy of
  each string, no duplication anywhere.

The geometry was read live, never assumed: the harness printed
`live VI geometry is 480x237` on every one of 5,272 swaps in this run.

Vertical coherence on this menu frame reproduces the stride signature the
striping measurement found:

| stride | vertical coherence |
|-------:|-------------------:|
|    320 |              0.631 |
|    480 |          **0.877** |

Because 480/320 = 1.5, a wrapped read repeats content about 1.5x per apparent
row, and across successive sheared rows the eye reads it as roughly 2.5x
tiling — which is exactly the number B3 recorded.

**The capture bug is still live in two harnesses.** Both hardcode
`FB_WIDTH = 320` / `FB_HEIGHT = 240` in their own `capture_framebuffer`:

- `examples/wm2000-census/src/main.rs` — in this repo, the harness that
  produced B3's observation.
- `packages/wm2000-boot/src/main.rs` in the separate `~/Code/recomps/wm2000`
  checkout — reported by the striping lane, never patched upstream.

The census harness now has an env-gated `WM2000_TRUE_GEOMETRY_DUMP=1`
companion dump that reads `fn64_abi::vi_width()` / `vi_output_height()` and
refuses to guess when the VI has not been programmed yet. The 320x240 path is
left untouched so the two readings stay comparable. Making true geometry the
default in both harnesses is the real fix and is not done here.
