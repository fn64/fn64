# The swap240 "striping" is a capture artifact, not a renderer defect

Measured from the 320x240 PNG the WM2000 harness wrote from guest RDRAM at
swap 240, and from the live ROM. That misread capture is no longer committed --
see [the frames README](README.md) for why it was retracted. The correct
capture is `wm2000-swap240-true-geometry-480x237.png`.

## 1. The pattern is period 3 at 1/3 density, not period 2 at 1/2

Non-black pixels per row:

- rows 0..=59: 319 or 320 (essentially full width)
- rows 60..=239: repeating 80, 159, 79 -- period 3, averaging 106 = 320/3

An interlace / field-parity bug predicts period **2** at density **1/2**. The
measurement is period **3** at density **1/3**. Interlace does not fit, and
neither does anything else that drops alternate rows.

**And WM2000 is not interlaced at all.** Its measured VI STATUS word is
`0x00013202` (pinned in `vi_scanout/tests.rs`'s `wm2000_measured_status`,
reconciled there against the literal a live run printed). Bit 6 -- RT64's
`serrate`, `rt64_vi.h:35`, the interlace bit -- is **0**. So the field-parity
hypothesis is ruled out twice over: by the shape of the pattern, and by the
register that would have had to be set for it to be possible.

Worth noting for the next reader: RT64's own interlace handling *halves the
width* when `serrate` is set (`rt64_vi.cpp:99-106`) -- it never drops
alternate rows. Even a genuinely interlaced N64 framebuffer would not produce
"every other row is black" through this path.

## 2. In linear RDRAM order it is a clean 480-pixel period

Run-length encoding the non-black mask from row 60:

    (1,80) (0,320) (1,79) (0,1) (1,80) (0,320) ...

The `(0,1)` is one genuinely black content pixel, so the structure is
**160 pixels written per 480 pixels of destination**.

## 3. The buffer's true row stride is 480, measured

Reinterpreting the *same* RDRAM pixel sequence at candidate strides and
scoring vertical coherence (fraction of vertically adjacent pixels that are
equal -- high for a coherent image, chance-level for a sheared one):

| stride | vertical coherence |
|-------:|-------------------:|
|    160 |              0.730 |
|    320 |          **0.536** |
|    480 |          **0.967** |
|    640 |              0.508 |
|    960 |              0.959 |

Stride 320 is chance level. Stride 480 is 0.967. (960 scores well only
because it is a multiple of 480.) Reshaping the committed frame to 480 wide
resolves it into clean rectangular regions with no striping whatsoever.

## 4. The ROM confirms it directly

Probing `plan_render_target_rows` over a real run: **39,543 rectangles, every
one with `img_width=480`, `bpp=2`**, with x reaching 479 and y reaching 239.

Probing the VI registers at swap time:

    [vi-probe] swap #3: vi_width=Some(480) vi_output_height=Some(237)

WM2000 renders into a **480x237 RGBA5551** framebuffer.

## Root cause

`capture_framebuffer` in the WM2000 harness
(`packages/wm2000-boot/src/main.rs`) hardcodes `FB_WIDTH = 320`,
`FB_HEIGHT = 240`. Reading a 480-stride buffer as 320-wide reads only the
first two thirds of it and shears every row by 160 pixels, which produces
exactly the observed 80/159/79 period-3 pattern.

`fn64-abi`'s own `vi_width()` doc comment already predicts this defect:

> A windowed presenter must use this as the framebuffer read stride rather
> than assuming a fixed width, or non-320-wide modes present sheared/offset.

and `vi_output_height()` names WM2000's 237 explicitly.

## What is NOT wrong

- **`vi_scanout.rs`** derives its stride from `registers.width()` (VI WIDTH)
  at `vi_scanout.rs:429`, used at `pixel_address` (`:534`). Correct. Mutating
  it to a hardcoded 320 is killed by 10 tests, including
  `the_source_stride_advances_rows_not_the_output_width` and four
  reference-oracle agreement tests.
- **The CPU compose path** derives its row stride from `image.width()` (the
  RDP `SetColorImage` register) at `raw_dpc/mod.rs:1752`. Correct. Mutating
  it to a hardcoded 320 is killed by ~14 tests across `production::tests`
  and `raw_dpc::tests`.

Both halves of the renderer already agree on 480 and already defend it.

## RT64 agrees

RT64 `5473732a` derives framebuffer geometry from VI_WIDTH and never assumes
320: `src/hle/rt64_vi.cpp:95-96` starts `fbSize()` from `{ width, 0 }`, and
`fbAddress()` at `:84` computes `rowBytes = width * (1U << (siz - 1))`.
RT64 also handles the interlaced double-stride case explicitly at
`rt64_vi.cpp:99-106` -- evidence that VI_WIDTH, not a constant, is the
authority for the row stride.

## The fix, and who owns it

The defect is in the **WM2000 harness**, not in fn64:
`~/Code/recomps/wm2000/packages/wm2000-boot/src/main.rs`, `capture_framebuffer`.

    const FB_WIDTH: usize = 320;
    const FB_HEIGHT: usize = 240;
    const FB_BYTES: usize = FB_WIDTH * FB_HEIGHT * 2;

should read the geometry the VI actually programs, which `fn64-abi` already
exports beside the `vi_swap_count()` the harness calls two lines away:

    let fb_width  = fn64_abi::vi_width().unwrap_or(320) as usize;
    let fb_height = fn64_abi::vi_output_height().unwrap_or(240) as usize;

Verified: with that change and no renderer change at all, the same ROM on the
same all-Rust stack dumps `docs/frames/wm2000-swap240-true-geometry-480x237.png`
-- 480x237, no striping, vertical coherence 0.958 versus 0.508 before.

This file is left in fn64 because the measurement and the RT64 citations
belong with the renderer; the one-line change belongs to the harness repo,
which was deliberately not modified from this lane.

## Not fixed here, and why

- The harness one-liner itself: `~/Code/recomps/wm2000` is a separate,
  currently dirty repo and outside this lane's scope. The change is written
  out above and was verified by patching the lane script's scratch copy.
- The renderer needed no change: both `vi_scanout.rs` and the CPU compose
  path already derive their stride from the right register, and mutation
  testing confirms both are guarded.
