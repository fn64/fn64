# WM2000 menu progression under scripted input

All frames are `reframed-*`: the harness dumps the guest framebuffer at a
hardcoded 320x240 while WM2000 scans out 480x237, so the raw dump shears every
row. `docs/tools/wm2000-reframe.py` re-lays the same bytes at stride 480,
recovering the top 160 true rows unsheared.

Input schedule for every frame here (swap-indexed, through the real PIF seam):

```
1100..1110:1000                       # START -- leaves attract
1200..1210:8000 ... 2400..2410:8000   # A every 100 swaps
2500..2510:8000 ... every 60 swaps    # A sustained, out to swap 12000
```

| frame | what it shows |
|---|---|
| `reframed-swap-1700.png` | menu panels -- green/purple/cream rectangles, no portrait |
| `reframed-swap-1950.png` | **one** wrestler model appears in the purple panel |
| `reframed-swap-2300.png` | **two** wrestler models side by side -- a versus/matchup screen |
| `reframed-swap-2500-two-controllers.png` | the same versus screen with `WM2000_PORTS=2` |

Text glyphs do not render on any of these; the panels are flat colour and the
wrestler models are untextured flat-shaded polygons. That is the same surface
gap the attract-mode ring frames show.

## The versus screen is where input stops working

From swap ~2500 to 5021 the screen never changes again, while the guest keeps
composing **3.00 display lists per field**. It cycles a **40-swap loop** of 38
unique frames (one held for three swaps), byte-identical on every repetition
across 2,500 swaps. That is a healthy screen idling on a condition, not a
stalled or crashed one.

`reframed-swap-2500-two-controllers.png` is the refutation of the player-2
hypothesis: with a second controller plugged in via
`fn64_abi::set_controller_port_state`, the plateau frames are **byte-identical**
to the one-controller run (117/117 identical over swaps 2400-2516, same 38
distinct frames). The second controller changes guest timing slightly
(`sim_time` and `gfx_tasks` diverge by one) but changes nothing on screen.
