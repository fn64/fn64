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
