# The 0x80120854 lookup trap is caused by the A press at swap 1900

`RT64-WM2000-INPUT-GRAMMAR.md` records the swap-~1900 abort as
nondeterministic, after refuting three explanations (shared trace file, host
memory, concurrency). It is none of those. It is **deterministic and
input-caused**, and the cause is one specific press in the standard lead-in.

## The measurement

Two runs of the prebuilt rs-lane binary, each with its own `WM2000_TRACE_PATH`,
run solo, one after the other.

| run | lead-in | outcome |
|---|---|---|
| `probeA` | START@1100, then **A every 100 swaps through 2400**, then A every 60 to 4500 | **abort at swap 1901**, `lookup: no recompiled function or host shim at vram 0x80120854` |
| `r_none` | START@1100, then **A every 100 swaps, stopping at 1800**, nothing after | **rc=0, reached swap 3147**, zero occurrences of `0x80120854` |

The abort log's own input trace fixes the cause to a single press:

```
[wm2000-input] swap #1800: pad0 -> buttons=0x8000 stick=(0, 0)
[wm2000-input] swap #1810: pad0 -> buttons=0x0000 stick=(0, 0)
[wm2000-input] swap #1900: pad0 -> buttons=0x8000 stick=(0, 0)
   -> swap #1901 panic: lookup ... 0x80120854
```

Every A press from 1200 through 1800 is harmless. The press at **1900** is
followed one swap later by the trap. Removing that press -- and every press
after it -- removes the trap and the run continues 1,246 swaps further.

## What this changes

The doc's framing was "different input schedules reach it at different swaps".
The sharper statement the measurement supports is: **the screen the guest is on
after ~1810 answers A by calling into `func_80120854`, and the earlier screens
do not.** The route around the gap is therefore not a matter of luck or timing:
it is "do not press A on that screen".

`r_none` also holds a sustained **3.00 gfx tasks/swap from swap 2182 through
3147** -- the composing state, not the 1.00 one-static-list plateau the doc
records for its control run. Reached without pressing anything after swap 1810.

## Caveat, stated plainly

Both runs used the **prebuilt** binary at
`/private/tmp/wm2000-probe-run/scratch/sib/...`, which links an fn64 tree at
`adb5c74a` -- 40 commits behind this branch, and **before the texture rung**.
Its framebuffer dumps show only 5 distinct hashes across 3147 swaps and are
black after ~swap 1200, consistent with the pre-rung frozen-frame regime the
doc describes and NOT with the post-rung 1,118-distinct-frames measurement.
The routing finding above is about which guest code path a press reaches and
does not depend on the renderer; the *frames* from these runs are not evidence
about the current tree.

## Re-run on THIS tree: the frames are not black, and the ring renders

The caveat above is now closed by rebuilding the harness against this branch
(the emitted crate re-emitted from `recompile_rom`, `packages/wm2000-boot`
patched to dump at true VI geometry via `fn64_abi::vi_width()` /
`vi_output_height()` instead of a hardcoded 320x240). Same lead-in
(`r_none.script`), 600,000 steps.

| | prebuilt (fn64 `adb5c74a`) | rebuilt (this branch) |
|---|---|---|
| dump geometry | 320x240 (sheared, 2/3 of the frame) | **480x237** (true) |
| max swap, rc | 3147, rc=0 | **3757, rc=0** |
| `0x80120854` | 0 occurrences | 0 occurrences |
| distinct frame hashes | **5** across 3147 swaps | **~3750** across 3757 swaps |
| frames after swap ~1200 | all black | rendered content throughout |

Five frames retained in operator scratch are from the rebuilt run and were
looked at:

- **`fn64-fb-700.png`** -- a **wrestling ring**: three dark-red ring ropes
  strung between turnbuckle posts, a pale-green canvas mat below them, and a
  large untextured yellow form (a wrestler) at the left.
- **`fn64-fb-900.png`** -- a **wrestler figure seen from behind**, legs in
  trunks and boots, torso, arms with wristbands, standing between green arena
  structures on a blue entrance ramp.
- **`fn64-fb-1005.png`** -- the same entrance shot with an arena/crowd
  backdrop, heavily texture-corrupted (striped garbage across the crowd rows).
- **`fn64-fb-1900.png`** and **`fn64-fb-3757.png`** -- the screen the run
  settles on and holds: flat coloured rectangles (a menu whose text and
  portraits are untextured quads) plus a slowly rotating yellow polyhedron.

**What this is, stated honestly: attract mode.** The ring and the wrestler are
the ROM's own demo/entrance sequence playing on its own around swaps 700-1100,
not a match this lane drove into. From ~1900 the run holds a single static
menu-shaped screen. Textures are largely absent or corrupt throughout, so
nothing here reads as a playable match: no health/energy bars, no HUD.
