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

---

## Update: the XOR4 fix landed, and the screen still shows noise

A second cited fix has now landed downstream of the texel-scale one: the
odd-row XOR4 bank exchange, where the LoadBlock writer derived its parity from
`source_t.raw()` while the reader derived its own from `low_t.integer()` -- a
different field in a different unit, disagreeing in 256 of 512 enumerated
cases. Full derivation and citation in
[`RT64-WM2000-TEXEL-LOCALISATION.md`](RT64-WM2000-TEXEL-LOCALISATION.md).

**CONFIRMED, by running the ROM on the fixed branch and looking:** it is not
sufficient either. `docs/frames/wm2000-after-xor4-fix-swap5192.png` (5,160
frames dumped to swap 5,021, zero panics, zero backend errors) shows correct
wrestler silhouettes and correct ring structure under dense magenta/green/black
per-pixel speckle. Same signature as before.

So the count is now **two real, hardware-cited, individually insufficient
fixes**. The working hypothesis for whoever picks this up: the noise has more
than one contributing cause, and single fixes will keep failing to cross the
visibility threshold until the last one lands. That is the argument for
finishing the parity corpus rather than continuing to fix-and-look -- see that
doc's "next concrete step", which is a specific command with specific expected
texel values and a named diagnosis for each wrong answer.
