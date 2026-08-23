# Task 38 — VI overscan crop: ESCALATED (col 479 is digitally scanned)

## Verdict up front

**Escalated, per the brief's explicit stop condition.** I implemented the
geometry-derived crop exactly as the brief specified, then MEASURED WM2000's
live VI geometry — and the honest finding is that **no guest-side VI geometry
excludes column 479**. The guest programs col 479 as a digitally scanned-out
column; it is hidden on real hardware only by *analog TV overscan*, which
cannot be derived from the VI registers and requires an overscan
convention/constant the brief told me not to hardcode. I have NOT committed a
guessed crop. Owner decision requested (message sent to `main`).

## Measured live geometry (my own bounded capture, not inferred)

Printed from a temporary VIDBG line in the shell present path during a bounded
`FN64_PUMP_CENSUS` run (the temp diagnostic has since been removed):

```
H_START  = 0x006c02ec  -> active window 108..748 = 640 analog dots
X_SCALE  = 0x00000300  -> x_step = 768/1024 = 0.75, x_offset = 0
VI_WIDTH = 480          (framebuffer line stride)
```

`0x006c02ec` is the **standard libultra NTSC H_START window**, not a narrowed
custom one.

## The derivation, and why it yields NO crop for WM2000

The VI horizontal scanout maps analog output dot `out` (0..output_width) to
framebuffer column `floor((x_offset + out*x_step)/1024)`, clamped to
`stride-1` — the exact mapping `fn64_render`'s `AxisSample::from_output` uses.
The last framebuffer column the geometry addresses is therefore

```
last = (x_offset + (output_width-1)*x_step) >> 10
visible_width = min(stride, last + 1)
```

For WM2000: `last = floor(639 * 0.75) = floor(479.25) = 479`, so
`visible_width = min(480, 480) = 480`. **Both** derivation routes the brief
named agree:

- **x_scale-clamp route:** output dot 639 maps to source col 479 -> col 479
  IS scanned. No crop.
- **H_START-vs-standard route:** WM2000 uses the *standard* 640-dot window,
  so there is nothing non-standard to crop.

Col 478 is reached by output dots 637–638; col 479 is reached by exactly one
dot (out=639, the extreme right), squarely inside where a CRT's overscan
begins. So col 479 is "scanned but never displayed on a TV" — analog
overscan, not a digital-window property.

## Pixel evidence (col 479 IS stale — task-37 reconfirmed)

From my capture (cols 476–479 across a spread of frames), cols 476→477→478
are scene-coherent (mean |Δluminance| ≈ 4–9) while **col 479 jumps hugely**
(Δ = 112–228). Decisive example: a frame whose cols 476–478 are uniform white
`(247,247,247)` has col 479 mostly black `(0,0,0)`. That is stale/uncovered
RDRAM, exactly candidate (b) from task-37. So the *symptom* is real and
matches; only the *mechanism* (analog overscan, not guest geometry) differs
from the crop-derivation the brief assumed.

## What is implemented and staged (currently a no-op for WM2000)

All geometry-driven, no hardcoded `width-1`:

- `crates/fn64-runtime/src/device/fabric.rs` — `DeviceFabric::vi_visible_width()`:
  derives the scanned-out column count from H_START + VI_X_SCALE + VI_WIDTH
  (the mapping above). Returns the full stride (no crop) when the geometry
  reaches the last column, as WM2000's does.
- `crates/fn64-abi/src/vi.rs` — `vi_visible_width()` accessor exposing it to
  the shell.
- `crates/fn64-shell/src/main.rs` — present path crops each line to
  `vi_visible_width` (dst_width narrower than src_stride; stride unchanged so
  cols 0..N are byte-identical). For WM2000 this equals the stride, so the
  present is unchanged — correct, because the geometry says col 479 is
  scanned.
- Unit tests:
  - `fabric.rs`/`device_b.rs`:
    `vi_visible_width_derives_the_scanned_out_column_count_from_geometry`
    (independently re-derives the visible width; covers narrow, stride-clamp,
    and zero-x_scale cases). PASS.
  - `framebuffer.rs`:
    `cropping_the_overscan_column_leaves_the_kept_columns_identical`
    (proves a narrower dst_width keeps cols 0..N pixel-identical and never
    reads the overscan column).

## Decision requested from owner

Cropping col 479 is hardware-faithful to a TV but is **not derivable from
guest VI geometry** — it needs a TV-overscan basis (a fraction/constant or a
reference such as RT64's). Options put to the owner:

- **A)** Crop by a TV-overscan fraction (source needed; may imply symmetric
  L/R/T/B overscan, not just col 479).
- **B)** Crop only extreme-right single-dot columns (out=639 -> col 479);
  geometry-adjacent but ad hoc.
- **C)** Clear col 479 each frame (task-37's fill-seam alternate) so it is
  black not stale — not a crop; touches the present buffer, not RDRAM.
- **D)** Different approach / accept.

Once the owner picks a basis I will derive it, verify before/after on live
frames (col 479 fixed; cols 0–478 pixel-identical), run the wgpu suite +
`check_rt64_parity.py`, and commit.

## No commit made

The escalation condition fired; committing a guessed overscan constant would
violate the brief. Awaiting the owner's basis.
