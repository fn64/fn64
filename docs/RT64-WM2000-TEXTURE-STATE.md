# LOOKED AT THE SCREEN after the texel-scale fix

The handoff's step 4 said "look at the screen after the texel-scale fix lands"
because nobody had. This is that check, done, with the frames committed.

Run: `FN64=/private/tmp/fn64-play-window`, rs lane + `FN64_RENDER=wgpu`, two
controllers, the committed match lead-in, 2M steps. 594 frames dumped at the
VI's real 480x237. Zero panics. `wm2000-frames.py` reports **ADVANCING** --
every one of the last 20 frames a distinct hash, so this is live animation, not
a held screen.

The measured frames at swaps 560 and 596 are retained in operator scratch
only because they contain game output.

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
sufficient either. The operator-scratch frame after the XOR4 fix (5,160 frames
dumped to swap 5,021, zero panics, zero backend errors) shows correct
wrestler silhouettes and correct ring structure under dense magenta/green/black
per-pixel speckle. Same signature as before.

So the count is now **two real, hardware-cited, individually insufficient
fixes**. The working hypothesis for whoever picks this up: the noise has more
than one contributing cause, and single fixes will keep failing to cross the
visibility threshold until the last one lands. That is the argument for
finishing the parity corpus rather than continuing to fix-and-look -- see that
doc's "next concrete step", which is a specific command with specific expected
texel values and a named diagnosis for each wrong answer.


---

# CLOSED: the texel values are correct on screen

**CONFIRMED by a ROM run and a human reading the frame.** The defect this
document bounded -- "coordinates are right, the fetch at those coordinates
is not" -- is fixed.

The cause was NOT in the TMEM address computation, the tile descriptor, the
palette, or the byte-lane mapping inside the sampler; all four were
investigated and cleared (see `RT64-WM2000-TEXEL-LOCALISATION.md`). It was
one layer earlier: the deferred guest-read capture handed the renderer RAW
ABI STORAGE bytes where `CapturedGuestRead`'s contract is N64-LOGICAL
bytes, so every 32-bit word of texture source reached the sampler
byte-reversed. Fixed in `9168bb9a` on both the production and conformance
paths.

Evidence, same scene; both filenames label frames retained in operator scratch:

| | image | what it shows |
|---|---|---|
| before | `wm2000-after-xor4-fix-swap5192.png` | dense magenta/green/black speckle; silhouettes only |
| after | `wm2000-after-byte-lane-fix-swap4090.png` | skin tones, facial detail, legible "AUSTIN 3:16" shirt print, shaded arena trusses, readable "Single Match" / "STEVE AUSTIN VS STEVE AUSTIN" / "RAW IS WAR" |

Zero panics and zero backend errors across 4,200+ dumped frames.

**The remaining items this document grouped with the noise** -- colour bands
and blocky glyphs -- were explicitly NOT scoped separately, on the grounds
that they plausibly shared this cause. Re-check them against a current frame
before scoping; do not assume they survived.


---

# CONFIRMED: the `PLANE_TO_TEXEL` fix did not regress the screen

A full ROM run after the fix (`rc=0`, 4,080 swaps, 9,982 gfx tasks, **zero
panics and zero backend errors** across 4,198 dumped frames) renders frame
4090 -- the same swap as the operator-scratch byte-lane-fix frame --
indistinguishable from it:
correct skin tones, facial detail, legible orange "AUSTIN 3:16" shirt
print, shaded steel trusses, readable "Single Match" / "STEVE AUSTIN VS
STEVE AUSTIN" / "RAW IS WAR".

So the constant that made the parity corpus agree with hardware and RT64
is safe on real content. Worth stating because it is a live-rasterizer
change, and the corpus alone could not have told us that.

# Open, separate: a scene-specific colour cast

**CONFIRMED pre-existing, and NOT caused by the `PLANE_TO_TEXEL` fix.**

At in-match swap 644 the frame shows green skin and a magenta WWF logo
where red and white belong. The same swap from the run BEFORE that fix is
pixel-identical, so the sampler change is not responsible. Frames from the
same post-fix run at the entrance scene (swap ~400) show correct skin
tones, black trunks and a legible crowd/arena backdrop, so this is not a
global colour break either.

That makes it scene-specific: something this in-match scene exercises and
the entrance scene does not. The likely area is the combiner or the
environment-colour path rather than the texture sampler, since the texels
themselves clearly resolve into imagery.

**Method note worth keeping.** The comparison that settles this needs the
SAME swap from both runs. Comparing the new in-match frame against the
older INTRO frame would have shown a difference that is entirely explained
by the scene changing, and would have blamed the wrong change.

---

# Hunting the colour cast: six hypotheses eliminated (2026-08-21)

The in-match green/magenta cast. Recorded so none of these is re-walked;
each died to a measurement, not an argument.

## The symptom, quantified

Sampling one scanline (y=180) of a cast frame:

| x | pixel | |
|---|---|---|
| 60 | `(107,165, 66)` | G>R — cast |
| 80 | `( 58, 66, 16)` | G>R — cast |
| **100** | **`( 82, 49, 25)`** | **R>G — CORRECT** |
| 200 | `(  8, 25,  8)` | G>R — cast |

Correct skin elsewhere: `(197,132,107)`, `(181,115,90)` — a clean R>G>B
ramp, G/R about 0.64-0.67. Cast skin: `(82,107,41)`, `(58,82,33)` — G>R>B,
with blue's ratio to the dominant channel about 0.5 in BOTH.

**Correct and cast pixels coexist in one frame on one scanline.** That single
fact kills every global-transform explanation.

## Eliminated

1. **Whole-frame colour averages as the instrument.** Frames 3000-5000 read
   "green-dominant", suggesting a transition near 2400-2600. Frame 2600 is
   the WrestleMania 2000 title logo, which is legitimately green on black.
   The probe was measuring scene content. **Averages cannot see this defect.**
2. **A global R<->G swap.** The numbers fit at first -- swapping R and G
   turns cast skin into a plausible ramp with blue untouched -- but a global
   swap would break every scene, and entrance scenes in the same run are
   correct.
3. **BGRA/RGBA confusion.** That is a B<->R swap, the wrong axis.
4. **The combiner.** The census names one dominant program, 2.38M draws:
   `0xfc15fea3 0xf00ff23f`, cycle 0 `(Texel0 - Zero) * ShadeAlpha + Zero`,
   cycle 1 `(Env - Combined) * Prim + Combined`. Instrumenting the draw path
   showed `env = [255,255,255,255]` and **`prim = [0,0,0,254]`** on every
   sample: with Prim = 0 the cycle-1 lerp weight is ZERO, so cycle 1 is a
   passthrough and Env never contributes. Cycle 0 reduces to
   `Texel0 * ShadeAlpha`, and shade alpha is SCALAR -- it darkens, it cannot
   shift hue. Two-cycle chaining is real (`two_cycle_first` =
   `two_cycle_second` = 2,859,904), so `Combined` is a genuine cycle-0 result.
5. **IA4 routed through a TLUT.** The affected draws sample
   `fmt=IntensityAlpha size=Bits4 lut=Rgba16`, which looks wrong -- IA4 is
   direct-colour. It is not: angrylion's
   `tlutswitch = (size << 2) | ((format + 2) & 3)` (`tex.c:116`) puts IA4
   (format 3, size 0) at case 1, the same nibble palette path as CI4
   (`tmem.c:1691`). fn64 matches hardware.
6. **The tile-0 hardcoding.** fn64 decodes each triangle's tile index
   (`triangle.rs:183`) and then drops it -- `RdpTriangleCommand` has no tile
   field -- so `TriangleDrawStateCollector` samples every draw through tile 0,
   where RT64 preserves it (`rt64_rdp.cpp:1088-1097`). A real divergence, but
   measured: **1,000,001 raw triangles, every one names tile 0.** Tiles 1-7
   are never used, so tile 0 IS the correct tile here.

Also checked and matching: the CI4/IA4 palette index. fn64's
`(palette << 4) | texel4` (`texel.rs:365`) is angrylion's
`p = (tpal << 4) | p` (`tmem.c:271`, `:953`) exactly. The `<< 2` elsewhere in
angrylion is byte-addressing of the entry, not the index.

## Two real defects found, neither of them this

- **The triangle tile index is dropped at the IR boundary.** Latent for
  WM2000, which only names tile 0, but wrong against RT64 and worth a fix
  plus a guard.
- **`TriangleDrawStateCollector` has drifted from `production.rs`'s
  `PlanCollector`.** The live path is `PlanCollector`, which carries the
  fixed 8-entry tile table; the duplicate still tracks tile 0 alone, despite
  `production.rs:787` stating "if `TriangleDrawStateCollector` changes, this
  file's own copy must be updated to match."

## RESOLVED: there is no defect. The oracle renders the same green.

The differential that should have been run first: render the SAME scene
through RT64 and compare.

| swap | RT64 (oracle) | wgpu |
|---|---|---|
| 443 | correct skin tones | correct skin tones |
| 644 | `(90,186,90)` `(76,95,35)` `(118,182,70)` | `(82,107,41)` `(58,82,33)` |

**At swap 644 the ORACLE is green-dominant too**, and at swap 443 both lanes
are correct. The operator-scratch RT64 capture for the "cast" scene shows the
same green skin.

So the in-match green is **WM2000's own rendering** -- a green-lit arena --
faithfully reproduced by both renderers. There was never an fn64 defect
here.

## What the nine eliminations were actually worth

They were not wasted: each verified a real fn64 path against hardware, and
two genuine defects surfaced on the way (the dropped triangle tile index and
the drifted `TriangleDrawStateCollector`). But the whole hunt could have
been avoided by one differential run at the start.

**The method lesson, which is the durable output here: when a symptom is
"the picture looks wrong", compare against the oracle BEFORE forming
hypotheses about the subsystem.** Nine subsystem-level eliminations could
not distinguish "fn64 is wrong" from "the game looks like that" -- only the
side-by-side could, and it took one run.
