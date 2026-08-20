# LOOKED AT THE SCREEN after the texel-scale fix

The handoff's step 4 said "look at the screen after the texel-scale fix lands"
because nobody had. This is that check, done, with the frames committed.

Run: `FN64=/private/tmp/fn64-play-window`, rs lane + `FN64_RENDER=wgpu`, two
controllers, the committed match lead-in, 2M steps. 594 frames dumped at the
VI's real 480x237. Zero panics. `wm2000-frames.py` reports **ADVANCING** --
every one of the last 20 frames a distinct hash, so this is live animation, not
a held screen.

Frames: `docs/frames/wm2000-after-texel-scale-fix-swap560.png` and
`...-swap596.png`.

## What changed, and it is real

**Before:** surfaces were flat, solid colour -- 87.5% of textured triangles
sampled exactly one texel.

**After:** surfaces carry dense per-pixel variation. The sampler is reading
varied texels, exactly as the per-triangle measurement predicted (one-texel
triangles 87.5% -> 12.0%). **The fix did what it claimed at the pixel level.**

## What it does NOT look like: correct textures

The variation is **noise**, not imagery -- speckled multi-coloured pixels across
the mat, ropes and wrestler bodies. So the texel-scale constant was necessary
and is not sufficient.

**What is clearly right in the frames:** geometry and shading. At swap 560 the
wrestler's arms, legs and torso are distinguishable with correct shape and
lighting; ring ropes and mat are correctly placed and coloured. The scene is
structurally sound. It is the texel *values* that are wrong.

## What this means for the next step

The remaining defect is now well-bounded, which it was not before:

- Coordinates are no longer collapsed (measured, and visible as variation).
- The values fetched at those coordinates are wrong.

So the suspect list shifts to what turns a coordinate into a texel: the TMEM
address computation, the tile descriptor's format/size/line fields, the
palette, or the byte-lane mapping into TMEM. Any of those produces exactly this
signature -- correct geometry, correct sampling *rate*, wrong colour per pixel.

**Do not scope the colour bands or blocky glyphs as separate work yet.** The
handoff flagged they might share the texel cause; they plausibly still do, and
the same investigation covers them.

**This is where the parity corpus gap bites.** The corpus is fill-rectangles
only. A textured-triangle case in it would localise this in seconds against
RT64 rather than requiring a 20-minute ROM run and a human looking at a PNG.
Extending the corpus (handoff step 3) is now clearly the cheaper path to the
next fix, not a detour before it.
