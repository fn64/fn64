# RT64 live antialiasing experiment

Status: experiment harness; no preferred visual policy selected.

## Question

Compare four independent RT64 paths without rebuilding or restarting the
guest:

1. native 1x, no MSAA;
2. 2x internal width and height, no explicit downsample;
3. 2x internal width and height, box-downsampled 2x;
4. native 1x with MSAA4x.

Two times each dimension is four times the internal pixel population. The
third mode is the supersampling experiment; the second distinguishes the
high-resolution renderer from its explicit downsample pass, while the fourth
distinguishes multisampling from both.

RT64's internal raster multiplier is independent of the post-VI presentation
geometry. On the WM2000 shell all four presets therefore continue to capture
320x237 and Pixels scales that image to the host window. The expected visual
differential is edge/filter quality, not a larger capture. Compare model
silhouettes and diagonal geometry; unchanged UI sprites do not reject the
internal-resolution path.

## Run

Build the WM2000 shell with its existing private inputs and RT64 feature. F7
toggles directly between native and 2x box-downsampled supersampling; F8 cycles
all four presets above. A startup or reloaded TOML that exactly matches a
preset updates the same typed active-preset state, so the next key press cannot
silently start from a stale cursor. The window title names the active preset so
a visual comparison does not depend on access to the launching terminal. Every
successful transition prints
`mode=...`, the complete settings SHA-256, and
`framebuffers_discarded=...`. Wait for at least one complete heartbeat window
after each transition before comparing `pump_ms`, frame cadence, and audio
starvation; a window spanning two modes is not an A/B observation.

For arbitrary live values, copy
`recomps/wm2000/packages/wm2000-block-boot/rt64-aa.example.toml` outside the repository and
launch with:

```text
FN64_RT64_SETTINGS_FILE=/absolute/path/rt64-aa.toml ... wm2000-shell
```

The file is applied at startup. Edit it and press F6 to reload it. Parsing is
transactional: a missing field, unknown field, unknown enum value, or typed
numeric-range violation leaves the active renderer settings unchanged and
prints the error. Supported values are:

```text
resolution = "original" | "window_integer_scale" | "manual"
resolution_multiplier = 0.0 .. 32.0
downsample_multiplier = 1 .. 32
antialiasing = "none" | "msaa2x" | "msaa4x" | "msaa8x"
```

The file controls only those four axes. Every other RT64 user setting comes
from fn64's complete default settings image, so an old in-process experiment
cannot leak into a later reload.

## Existing mechanism evidence

`rt64_resolution_downsample_behavior` already applies native 1x, manual 2x,
and manual 2x/downsample 2x live and checks their distinct post-VI pixels.
`rt64_user_controls_rebuild_behavior` independently exercises the live MSAA4x
resource-rebuild path and exact active-policy identity. The keyboard/config
harness adds no new renderer algorithm; it makes those typed controls
available to the real interactive workload for visual and frame-time A/Bs.

## Validation on 2026-08-14

The first WM2000 visual run found a real integration defect: all four modes
were byte-identical because fn64 added RT64's conventional leading scanline to
VI_ORIGIN before crossing the native boundary, then pinned RT64 subtracted the
same row in `VI::fbAddress()`. That cancellation made presentation miss the
managed color targets at `0x0038f800`/`0x003c7c00` and create native 1x scratch
targets at the guest VI origins `0x0038fbc0`/`0x003c7fc0`. The same no-op was
reproduced at 1,200 fields without input and at 4,000 fields under the committed
entrance route: native and 2x-box hashes were respectively identical within
each checkpoint.

The adapter now preserves guest VI_ORIGIN and lets RT64 apply its lookup bias
once. A typed, explicit diagnostic waits both renderer workers and reports the
pre-normalization workload scale plus the exact target scale/raster/downsample
selected by the completed present. At the 1,200-field checkpoint, native now
selects `0x003c7c00`, scale 1x, raster 480x240, downsample 1, SHA-256
`51830b74...`; the existing 2x-box preset selects the same managed target at
scale 2x, raster 960x480, downsample 2, SHA-256 `5524bbd5...`. This is one
native baseline followed by ten consecutive clean 2x runs of the same compiled
artifact; every 2x run reproduced both the managed-target geometry and exact
distinct capture identity.

- The typed registered-renderer update seam and strict parser/preset tests
  passed ten consecutive clean runs.
- The live Metal resolution/downsample fixture passed ten consecutive clean
  runs on Apple M5 Pro. Its 32x32 discriminator changed exactly seven pixels
  between high-resolution 2x and explicit 2x box downsampling on every run.
- The live Metal MSAA/resource-rebuild fixture passed ten consecutive clean
  runs after its preflight identity was mechanically refreshed for the already
  merged `s2dex-object-rect:v3` overlay. Pixel and policy digests retained the
  gate's existing certified values.
- The private-input WM2000 shell compiled with RT64, and one zero-frame smoke
  launch applied the example config through the real shell -> ABI -> RT64
  path. That launch is a smoke check, not a ten-run or visual-quality claim.
- After correcting live-origin ownership, ten consecutive 1,200-field WM2000
  Metal runs selected the 960x480 managed 2x target and reproduced the exact
  `5524bbd5...` post-VI capture. The paired native checkpoint selected 480x240
  and produced distinct `51830b74...` pixels.
