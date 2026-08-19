# Measurement: the "striping" in wm2000-allguards-swap240.png

Measured from `docs/frames/wm2000-allguards-swap240.png` (320x240, the guest
framebuffer read out of RDRAM by wm2000-boot's `capture_framebuffer`).

## It is NOT a one-pixel row period

Non-black pixels per row:

- rows 0..=59: 319 or 320 per row (essentially full width)
- rows 60..=239: repeating 80, 159, 79 -- period 3, averaging 106 = 320/3

So the lower field is at exactly **one third** density, not one half, and the
period is 3 rows, not 1. An interlace/field-parity explanation predicts a
period of 2 and a density of 1/2. It does not fit.

## In linear raster order the lower field is a clean 480-pixel period

Run-length encoding the non-black mask linearly from row 60:

    (1,80) (0,320) (1,79) (0,1) (1,80) (0,320) (1,79) (0,1) ...

The `(0,1)` is one genuinely black content pixel, so the true structure is:

    160 pixels written, 320 pixels skipped, repeating -- period 480 pixels

At RGBA5551 (2 bytes/pixel) that is **320 bytes written every 960 bytes**.
A destination row of 320 pixels is 640 bytes. So the payload is half a row
and the advance is one and a half rows; payload:stride = 1:3.

## What this rules out

- Field parity / interlace: predicts period 2 and density 1/2. Measured
  period 3 and density 1/3.
- The WGSL sampler first-row parity fix (935c7e4a): that is a GPU sampler
  path and does not write RDRAM at all.
- `vi_scanout.rs`: it *reads* guest RDRAM to present. The dumped PNG is read
  straight from RDRAM by the harness, so vi_scanout is not in the causal
  path for what is in RDRAM.

The defect is an addressing/stride error in whatever writes composed pixels
back into guest RDRAM: a 1:3 payload-to-stride ratio with a 160-pixel payload.
