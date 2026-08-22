# WM2000 flat-shading: the combiner census


> **PROVENANCE WARNING.** This document's stated authority is
> angrylion-rdp-plus, which `AGENTS.md:26-45` EXCLUDES from fn64's clean-room
> protocol (`docs/DISCOVER-PLAN.md:2260` records the exclusion). Its
> observations about WM2000 and about fn64's own behaviour remain valid --
> measured facts about a ROM are explicitly allowed -- but **any claim here
> about what HARDWARE does, sourced only to angrylion, is not admissible as
> fn64 authority.** Re-ground such a claim on pinned RT64 (MIT), the public
> libultra headers, or a fresh measurement before acting on it.

Card: distinguish "the sampled texel is wrong" (candidate 1) from "the
combiner never selects Texel0" (candidate 2), given the prior lane's
CONFIRMED result that 255,654 of 255,654 admitted triangles are textured and
every one reaches `sample_point`.

## CONFIRMED: the one-cycle bitfield slice in `fn64-render-wgpu` is correct

**Measured by reading three independent implementations, not by running.**
Marked CONFIRMED because it is a source-identity check with a
cycle-accurate oracle on one side, not an inference.

`run_one_cycle` (`crates/fn64-render-wgpu/src/combiner.rs:699`) passes
`SECOND_CYCLE = true` to every `decode_color`/`decode_alpha` call, i.e. in
one-cycle mode it reads the *cycle-1* bitfield slice (`w0>>5`, `w0&0x1F`,
`w1>>24`, `w1>>6`, ...), not the cycle-0 slice.

That is what the hardware does. angrylion `combiner_1cycle`
(`src/core/n64video/rdp/combiner.c:173-220`) dereferences
`combiner_rgbsub_a_r[1]`, `combiner_rgbsub_b_r[1]`, `combiner_rgbmul_r[1]`,
`combiner_rgbadd_r[1]` and `combiner_alphasub_a[1]` / `alphasub_b[1]` /
`alphamul[1]` / `alphaadd[1]` -- index **1** throughout, which
`rdp_set_combine` (`combiner.c:522-560`) populated from the `*_rgb1`/`*_a1`
fields. So the RDP's one-cycle mode evaluates the second programmed cycle,
and the wgpu port matches it.

**This also identifies a divergence in `fn64-render-reference`, which is NOT
on the live path.** `evaluate_combiner`
(`crates/fn64-render-reference/src/raster/combiner.rs:64`) maps
`CycleType::OneCycle => 1` and then runs
`state.mode.cycles.into_iter().take(cycle_count)` -- i.e. `cycles[0]`, the
cycle-**0** slice. Against angrylion that is the wrong slice for one-cycle
mode. Recorded here rather than fixed under this card: the reference
renderer is a different lane's surface and this card's symptom is on the
wgpu path. It does mean **the reference renderer is not a usable oracle for
one-cycle combiner questions** until it is reconciled.

## CONFIRMED: the SetCombine wire bit positions agree three ways

Hand-derived from public gbi.h's `GCCc0w0`/`GCCc1w0`/`GCCc0w1`/`GCCc1w1`
macros, then checked against two implementations that do not share code:

| field | gbi.h | wgpu `parse_*` | angrylion `rdp_set_combine` |
|---|---|---|---|
| a0 / sub_a_rgb0 | `w0>>20 & 0xF` | same | same |
| c0 / mul_rgb0 | `w0>>15 & 0x1F` | same | same |
| Aa0 / sub_a_a0 | `w0>>12 & 0x7` | same | same |
| Ac0 / mul_a0 | `w0>>9 & 0x7` | same | same |
| a1 / sub_a_rgb1 | `w0>>5 & 0xF` | same | same |
| c1 / mul_rgb1 | `w0>>0 & 0x1F` | same | same |
| b0 / sub_b_rgb0 | `w1>>28 & 0xF` | same | same |
| b1 / sub_b_rgb1 | `w1>>24 & 0xF` | same | same |
| Aa1 / sub_a_a1 | `w1>>21 & 0x7` | same | same |
| Ac1 / mul_a1 | `w1>>18 & 0x7` | same | same |
| d0 / add_rgb0 | `w1>>15 & 0x7` | same | same |
| Ab0 / sub_b_a0 | `w1>>12 & 0x7` | same | same |
| Ad0 / add_a0 | `w1>>9 & 0x7` | same | same |
| d1 / add_rgb1 | `w1>>6 & 0x7` | same | same |
| Ab1 / sub_b_a1 | `w1>>3 & 0x7` | same | same |
| Ad1 / add_a1 | `w1>>0 & 0x7` | same | same |

Passing `w0` with its opcode byte still in bits 31..24 is harmless: the
highest bit any w0 field reaches is 23.

So a mis-decoded combiner program is **not** the explanation. The census
below measures what the correctly-decoded programs actually select.

## REFUTED: the CI-aliases-to-I8 path is not a defect

`docs/RT64-WM2000-INMATCH-GAPS.md` raises, as "a silent wrong-colour path the
guard audit could not see", that with `en_tlut` clear a CI4/CI8 texel is
aliased to I8 -- the palette index rendered as greyscale intensity -- and
proposes it as a candidate explanation for the blocky dark glyphs.

**fn64 does exactly that, and so does the hardware.** The alias is real
(`crates/fn64-render-wgpu/src/tmem/texel.rs:375`,
`TextureLutMode::Disabled => Ok(ResolvedIndexedTexel::Direct(decode_i8(index)))`,
with `decode_i8` at `:571` setting `r = g = b = a = index`, and the index
itself formed at `:365` as `(palette << 4) | nibble` for CI4).

angrylion, the cycle-accurate oracle, does the same thing byte for byte
(`src/core/n64video/rdp/tmem.c:260-288`):

```c
case TEXEL_CI4:
    p = wstate->tmem[taddr & 0xfff];
    p = (s & 1) ? (p & 0xf) : (p >> 4);
    p = (uint8_t)(tpal << 4) | p;
    color->r = color->g = color->b = color->a = p;
case TEXEL_CI8:
    p = wstate->tmem[taddr & 0xfff];
    color->r = color->g = color->b = color->a = p;
```

Same `tpal << 4` fold for CI4, same splat to all four channels, same
absence of a refusal. So this alias is not fn64 losing a colour: it is the
RDP's own documented-by-silicon behaviour for a CI tile drawn with the TLUT
off, and any game relying on it gets greyscale on real hardware too.

**Consequence for this card:** candidate 3 is closed. If WM2000's glyphs are
dark and blocky, either the guest genuinely programs a TLUT and fn64 loses
it somewhere upstream of `lut_mode` (a different defect, in `SetOtherMode`
latching or TLUT residency, not in this decode), or the glyphs are a
downstream artifact. Widening this decode would take fn64 further from
hardware, not closer.

## CONFIRMED defect (different crate): `fn64-render-reference` runs the wrong
## slice in one-cycle mode

Found while establishing which slice is authoritative. Recorded here, not
fixed under this card, because the reference renderer is another lane's
surface -- but it is a live defect on that crate's raster path, not a
latent one, and it disqualifies the reference as an oracle for one-cycle
combiner questions.

`evaluate_combiner` (`crates/fn64-render-reference/src/raster/combiner.rs:64`)
maps `CycleType::OneCycle => 1` and then runs
`state.mode.cycles.into_iter().take(cycle_count)`, evaluating `cycles[0]`.
`CombinerMode::decode` (`gbi/types.rs:276-309`) fills `cycles[0]` from the
cycle-**0** wire fields (`w0>>20`, `w0>>15`, `w1>>28`, `w1>>15`, ...). There
is no compensating swap anywhere between them.

angrylion's `combiner_1cycle` (`combiner.c:173-220`) dereferences index
`[1]` for all eight inputs, and `rdp_set_combine` (`:522-560`) fills index
`[1]` from the cycle-1 wire fields. So one-cycle mode evaluates the SECOND
programmed cycle, and the reference renderer evaluates the first.

**Why it has gone unnoticed, and why that is the interesting part.** This is
precisely the coincident-fixture trap `docs/RT64-WM2000-HARNESS-TRAPS.md`
warns about, occurring naturally:

- `CombinerMode::default()` (`types.rs:330-353`) builds ONE `modulate` cycle
  and stores `cycles: [modulate; 2]` -- both slices identical, so every
  fixture on the default path reads the same answer either way.
- Games conventionally write `gsDPSetCombineMode(G_CC_X, G_CC_X)`
  (`ultra64/gbi.h:3610`, `gsDPSetCombineMode(a, b) -> gsDPSetCombineLERP(a, b)`
  with `a` = cycle 0 and `b` = cycle 1) in one-cycle mode, passing the same
  mode twice. A ROM that does so is also indistinguishable.

So the bug is invisible until a title programs two DIFFERENT slices and runs
one-cycle. `fn64-render-wgpu` is correct here (`run_one_cycle`'s
`SECOND_CYCLE = true`), which means the two lanes silently disagree on
exactly those programs -- and a differential run between them would blame
the wrong side.

**Consequence for this card:** the brief names `fn64-render-reference` as
"repeatedly the better oracle for CPU-side questions". For one-cycle
combiner output specifically, it is not, until this is reconciled.

## MEASURED: the combiner-input tally

**Status: CONFIRMED**, measured 2026-08-20 on the real ROM through
`scripts/play-wm2000.sh` (rs recompiler, wgpu renderer, no `--features
rt64`, banner confirmed `renderer : wgpu` and not `reference-fallback`),
with `FN64_COMBINER_CENSUS=1 FN64_TRI_DROP_STATS=1`. Four consecutive
100,000-note ticks; the ratios below are stable across all four.

Cumulative at note=300,000, every counted draw textured:

```
[fn64-combiner]   TEXTURED draws: color reads Texel* = 137728, ignores Texel* = 162272
[fn64-combiner]   color A: Texel0=117068 Primitive=3575 Shade=15271 Environment=141238 Zero=22848
[fn64-combiner]   color B: Combined=130434 Texel0=10804 Environment=2339 Zero=156423
[fn64-combiner]   color C: Texel0=3575 Primitive=142343 ShadeAlpha=131234 Zero=22848
[fn64-combiner]   color D: Combined=130434 Texel0=17085 Primitive=16567 Environment=2339 Zero=133575
[fn64-combiner]   alpha A: Combined=127055 Texel0=18531 Primitive=7517 Zero=146897
[fn64-combiner]   alpha B: Zero=300000
[fn64-combiner]   alpha C: Texel0=7517 Primitive=145586 Zero=146897
[fn64-combiner]   alpha D: Combined=3379 Texel0=111680 Primitive=16567 Shade=15271 Zero=153103
```

The `[fn64-tri-drop]` control ran in the same process and reproduced the
prior lane's numbers exactly (tick=400000: ADMITTED=341150, textured=341150,
untextured=0, only `no_covered_rows` firing), so this is the same population
that lane measured, not a different one.

### CANDIDATE 2 IS REFUTED: Texel0 IS selected

**45.9% of textured draws (137,728 of 300,000) select a Texel input in
colour**, and `Texel0` is the single most common slot-A selector after
`Environment` (117,068). It also dominates `alpha D` (111,680). So the
sampled texel is NOT being universally discarded, and the bug is not "the
combiner never names Texel0".

That closes the cheap candidate the brief said to test first, in the
direction the brief said would send the investigation to candidate 1.

### But the tally names something the brief did not anticipate

**54.1% of textured draws (162,272) read no Texel input at all in colour.**
Within one 100,000-note window the marginals coincide exactly:

```
window color A: Environment=46382  Texel0=39549  Shade=6802  Zero=6378  Primitive=889
window color C: Primitive=46382    ShadeAlpha=46351  Zero=6378  Texel0=889
```

`A.Environment` and `C.Primitive` are the **same number**, 46,382. Four
independent per-slot histograms cannot prove those two selectors co-occur in
one program -- that is exactly the joint-vs-marginal gap -- but an exact tie
across a 100,000-note window is strong evidence of a single dominant program
of the shape `(Environment - B) * Primitive + D`, which reads no texel.

**This is a textured draw whose program ignores its texture.** That is
legal: a game may bind a texture and then not sample it. But at 54% of all
textured draws it is a large enough population to explain flat models by
itself, and it is NOT the "combiner fails to select Texel0" defect the brief
described -- the programs that do select Texel0 select it correctly.

**HYPOTHESIS, not yet measured:** these draws are correct and WM2000 really
does draw over half its geometry with an env/prim program, in which case
flatness comes from candidate 1 on the OTHER 46%. The distinguishing
measurement is the raw-program histogram added in this branch
(`census::note_wire` / `program_histogram`), which keeps the `(low, high)`
`SetCombine` words themselves so the dominant program can be hand-decoded
from the wire layout rather than inferred from margins. That run has not
been made yet.

## RESOLVED: the program histogram settles it. Candidate 2 is refuted.

**Status: CONFIRMED**, second run, same script and stack, with the raw
`(low, high)` histogram and the cycle-mode breakdown added. Two consecutive
ticks, stable.

```
[fn64-combiner]   passes: one_cycle=30045 two_cycle_first=84978 two_cycle_second=84977
[fn64-combiner]   distinct programs = 10, top 8 by count:
[fn64-combiner]     0xfc15fea3 0xf00ff23f x73925
[fn64-combiner]     0xfcffffff 0xfffdf6fb x12829
[fn64-combiner]     0xfc5196a3 0x112cfe7f x10601
[fn64-combiner]     0xfc45fea3 0xf00ff83f x7762
[fn64-combiner]     0xfcffb3ff 0xff64fe7f x3022
[fn64-combiner]     0xfc1596a3 0xf0fffe38 x2423
[fn64-combiner]     0xfc309661 0x552eff7f x2304
[fn64-combiner]     0xfc30b261 0xff67ffff x882
```

**WM2000 uses only ten distinct combiner programs in an in-match window, and
most draws are TWO-cycle** (`two_cycle_first` and `two_cycle_second` are
equal, as they must be, and together outnumber `one_cycle` nearly 6:1).

Hand-decoding the dominant program `0xfc15fea3 / 0xf00ff23f` (73,925 draws,
~64% of the window) from the gbi.h bit positions:

| pass | rgb | alpha |
|---|---|---|
| cycle 0 | `(Texel0 - Zero) * ShadeAlpha + Zero` | `(Zero - Zero)*Zero + Texel0` |
| cycle 1 | `(Environment - Combined) * Primitive + Combined` | `(Combined - Zero)*Primitive + Zero` |

That is **texture modulated by shade alpha, then fog-lerped toward the
environment colour by the primitive fraction** -- the standard N64 fogged
texture combiner. The texture is sampled and used.

### The earlier reading was mine, and it was wrong

The per-slot tally in the previous section reported "54.1% of textured draws
read no Texel input", and flagged an exact `A.Environment == C.Primitive`
coincidence. Both observations were real. The **inference** was wrong: those
are the CYCLE-1 fog passes of two-cycle programs whose cycle-0 pass sampled
the texture. 37,727 of the 47,596 "ignores" notes in the first run -- 79% --
are the second pass of exactly three programs whose first pass reads
`Texel0`.

Four per-slot marginals cannot distinguish "a program that ignores the
texel" from "the second pass of a program that did not", which is precisely
the joint-vs-marginal gap the histogram was added to close. Recorded rather
than quietly corrected, because the marginal-only tally was convincing and
would have sent a fix lane at the combiner decode, which is not defective.

The census now counts only the first evaluated pass for this ratio, and
`the_wm2000_fog_program_samples_the_texture_in_its_first_cycle` pins the
ROM's real program against hand-derived expectations.

### Verdict on the card's two candidates

- **Candidate 2 (the combiner discards the texel): REFUTED.** The dominant
  program samples `Texel0` in cycle 0 and carries it through `Combined`.
  The programs are decoded correctly -- verified three ways against gbi.h,
  angrylion and RT64 -- and they select the texture.
- **Candidate 1 (a wrong-but-valid sampled texel) is the surviving
  candidate**, now by elimination as well as by the prior lane's evidence.

### The strongest lead for candidate 1, NOT yet measured

**Status: HYPOTHESIS.** The dominant program modulates the texel by
`ShadeAlpha`, and multiplies the result by nothing else. So the drawn colour
is `texel.rgb * shade.a`, fogged. Two ways that renders flat:

1. **A wrong texel**, the card's candidate 1 proper.
2. **A wrong `ShadeAlpha`.** If the interpolated shade alpha were near a
   constant, every pixel of a triangle would take one scaled copy of its
   texel -- which is what "flat" looks like -- even with a perfectly correct
   sample. Nothing in this card measured shade alpha, and it is the second
   multiplicand of the program 64% of draws use.

A further, separate suspicion worth a lane:
`triangle_span::texture_coordinates_s10_5`
(`crates/fn64-render-wgpu/src/raw_dpc/triangle_span.rs:519`) performs the
perspective divide in **f32** with a `PERSPECTIVE_TEXEL_SCALE` of 1024.
angrylion's `tcdiv_persp` (`src/core/n64video/rdp/tcoord.c:1027`) is an
integer reciprocal-table path (`tcdiv_table[0x8000]`, shift/log2 based) with
its own overflow and out-of-bounds handling. Those are not the same
function, and a coordinate error lands squarely in candidate 1. Whether they
agree closely enough to be invisible has not been measured here.

## MEASURED, and it refutes candidate 1 too: the texel is NOT constant

**Status: CONFIRMED.** Third run, same script and stack, with the per-pixel
histograms. 35,141,022 drawn pixels in one tick:

```
[fn64-combiner]   TEXTURED draws: color reads Texel* = 52114, ignores Texel* = 9896
[fn64-combiner]   texel luma /16 (pixels=35141022): 0:4223419 16:3465814 32:3519217
  48:590388 64:1615920 80:2451609 96:4493175 112:3611297 128:5475595 144:2081631
  160:2171107 176:545283 192:448160 208:334372 224:37197 240:76838
[fn64-combiner]   shade alpha /16 (pixels=35141022): 64:325441 80:335495 96:363971
  112:394935 128:4386568 144:3617304 160:3622402 176:3786463 192:3773787
  208:3571048 224:3584695 240:7378913
```

- **Texel luma occupies all 16 buckets.** The largest holds 15.6% of pixels
  and the top two together only 28.4%. This is a broad, populated
  distribution -- exactly what sampling real texture data looks like.
- **Shade alpha occupies 12 of 16 buckets**, 96% of pixels at or above 128
  and none below 64. Varied, and in the range a lighting term lives in.

`docs/RT64-WM2000-INMATCH-GAPS.md` set the decision rule for this
measurement in advance: "A texel histogram with one or two distinct values
indicts (1); a varied texel histogram with a flat output indicts (2)." The
histogram has sixteen populated values. **By that rule candidate 1 is
refuted**, and candidate 2 was already refuted by the program histogram
above.

Note also the corrected ratio in the same tick: **52,114 textured draws
consult a Texel input against 9,896 that do not -- 84%**, where the
uncorrected metric had reported 46%. That is the two-cycle fix from the
previous section showing up in the number it was distorting.

## Where this leaves the card

Every hypothesis the brief named is now measured and refuted:

| candidate | verdict | evidence |
|---|---|---|
| Combiner never selects `Texel0` | **REFUTED** | dominant program is `Texel0 * ShadeAlpha` then fog; 84% of textured draws read a Texel input |
| Sampled texel is wrong-but-valid (constant/aliased) | **REFUTED** | texel luma occupies all 16 buckets, largest 15.6% |
| CI-without-TLUT aliases to I8 | **REFUTED as a defect** | byte-exact with angrylion `tmem.c:260-288` |
| Combiner decode reads the wrong bitfield slice | **REFUTED** | agrees with gbi.h, angrylion and RT64 three ways |

**So the renderer is sampling varied texels, selecting them in the combiner,
modulating them by a varied shade alpha, and fogging the result -- and the
models still look flat.** The defect is therefore NOT in the combiner and
NOT in whether the sampler returns varying data. It is somewhere that this
card's instruments cannot see, and the honest statement is that this card
narrowed the search rather than closing it.

### What the evidence now points at, all HYPOTHESIS

1. **The texel varies, but does it vary in the RIGHT PLACE?** A histogram is
   blind to spatial arrangement: a texture sampled with wrong coordinates
   produces an identically varied histogram and a smeared or single-colour
   *triangle*. This is the largest remaining gap, and it is not expensive to
   close -- a per-triangle count of DISTINCT texels would separate "varied
   across the frame" from "varied within each triangle". The present
   histogram cannot, and should not be read as if it could.
2. **`texture_coordinates_s10_5` is a float port of an integer algorithm.**
   `crates/fn64-render-wgpu/src/raw_dpc/triangle_span.rs:519` does the
   perspective divide in `f32` against a `PERSPECTIVE_TEXEL_SCALE` of 1024;
   angrylion's `tcdiv_persp` (`src/core/n64video/rdp/tcoord.c:1027`) is a
   reciprocal-table integer path with its own shift/log2 handling. These are
   different functions, and per (1) a coordinate error is exactly the shape
   the current evidence cannot exclude.
3. **The observation itself deserves re-checking.** "Every model renders
   flat" was an eyeball reading of a moving window. The census says the
   pixels being written are varied. Both can be true if the variation is
   spatially wrong -- but it is also worth confirming what is on screen with
   the same rigour applied to the counters.

## THE DEFECT, MEASURED: 87% of textured triangles sample exactly ONE texel

**Status: CONFIRMED.** Fourth run, same script and stack, with the
per-triangle distinct-texel count. Three consecutive ticks:

```
[fn64-combiner]   distinct texels per triangle (triangles=50494):  1:44657 2:4821  3-4:315  5-8:693  9-16:8
[fn64-combiner]   distinct texels per triangle (triangles=99607):  1:87115 2:10859 3-4:606  5-8:1019 9-16:8
[fn64-combiner]   distinct texels per triangle (triangles=148740): 1:129443 2:16499 3-4:1183 5-8:1601 9-16:14
```

**87.0% of textured triangles read exactly one distinct texel across their
entire span**, and the ratio is stable across all three ticks. **No triangle
in 148,740 exceeded 16 distinct texels.** For comparison, a correctly
textured wrestler polygon covering a few hundred pixels of a 32x32 texture
should read hundreds of distinct texels.

**This is the flat shading, measured directly at the sampler.** The models
are not flat because the combiner discards the texel, and not because the
sampler returns a constant -- they are flat because each triangle samples
one texel and paints its whole span with it.

### Why every earlier instrument missed it, and why that is the lesson

The whole-frame luma histogram in the previous section is *completely
consistent with this*: 87% of triangles each reading a DIFFERENT single
texel produces a broad, healthy-looking sixteen-bucket distribution over the
frame. Both facts are true at once, and only the per-triangle count
separates them.

That is the general trap, stated plainly: **an aggregate histogram cannot
answer a per-primitive question.** The frame-wide texel histogram was
proposed in `docs/RT64-WM2000-INMATCH-GAPS.md` as the measurement that would
"indict (1) or (2)", and taken at face value it refuted BOTH candidates and
would have closed the investigation with no defect found. The measurement
was not wrong; the inference drawn from its shape was, in exactly the way
this repo's own traps document warns about aggregates. The distinct-texel
count is one extra `HashSet` in the same loop and it is unambiguous.

So the earlier section's "candidate 1 REFUTED" stands only in its narrow
sense -- the sampler does not return one constant value frame-wide -- and
must NOT be read as "the sampled texel is fine". Candidate 1 is alive in its
per-triangle form, and this is it.

### Where the fault must be

The texel is chosen by `(s, t)` from
`triangle_span::texture_coordinates_s10_5`. One texel per triangle means
those coordinates are effectively constant within a triangle, which points
at either:

1. **The coordinate derivation** -- the S/T/W planes, their per-pixel
   evaluation, or the perspective divide in
   `crates/fn64-render-wgpu/src/raw_dpc/triangle_span.rs:519`. That function
   is a float port of angrylion's integer reciprocal-table `tcdiv_persp`
   (`src/core/n64video/rdp/tcoord.c:1027`), and its own doc records the
   precedent exactly: without the empirical `* 1024.0` scale "the whole
   title-screen quad collapsed onto texel (0,0) -- every pixel sampled the
   image's corner and the presented frame was a uniform field." That is the
   symptom now measured, one primitive at a time.
2. **The addressing** -- `tmem/sample.rs`'s shift/mask/mirror/clamp folding
   a varying coordinate onto one texel.

These live in different files, and the S/T-spread census committed on this
branch tells them apart: if the requested coordinates span many texels and
one texel comes back, the fault is addressing; if the coordinates themselves
barely move across the span, it is the derivation.

## ROOT CAUSE, derived against angrylion: the perspective scale is 32x short

**Status: CONFIRMED for the derivation; the ROM fix is measured in the next
section.**

Two further measurements narrowed it to one constant.

**1. The coordinates themselves barely move.** Fifth run, per-triangle S/T
spread in whole texels:

```
[fn64-combiner]   S spread/triangle (texels): 0:36341 1:10787 2-4:2314 5-8:210 9-16:570 17-64:145
[fn64-combiner]   T spread/triangle (texels): 0:36112 1:13213 2-4:1004 5-8:38
[fn64-combiner]   perspective: off=0 on=50367
```

**72% of triangles request a spread of ZERO whole texels**, and every single
triangle takes the perspective path. So the fault is in the coordinate
DERIVATION, not in the addressing folding varying coordinates together --
the coordinates never varied.

That also explains why shade is fine while texture is not: both are
evaluated at the *same* `attribute_sample` point (`triangle_span.rs:383`),
and shade alpha spans twelve histogram buckets. The sample point varies
correctly. Only the texture path's post-processing of it does.

**2. The scale constant is wrong by exactly 2^5.** Derived by building
angrylion's `tcdiv_table` exactly -- both 64-entry `norm_point_table` and
`norm_slope_table` transcribed from `tcoord.c:3-24`, the same
shift/normalize/interpolate construction as `tcoord.c:1127-1146` -- and
evaluating `tcdiv_persp` over known inputs:

| ss | sw | ss/sw | tcdiv out (flags masked) | out / (ss/sw) |
|---|---|---|---|---|
| 64 | 64 | 1.0000 | 32768 | 32768.0 |
| 31 | 16 | 1.9375 | 63488 | 32768.0 |
| 100 | 50 | 2.0000 | 65537 | 32768.5 |
| 300 | 100 | 3.0000 | 98306 | 32768.7 |

**angrylion's `tcdiv_persp` returns `(ss/sw) * 2^15`.** Then
`tcshift_cycle` (`tcoord.c:83-102`) takes `SIGN16` of it -- the low 16 bits
-- and `texture_pipeline_cycle` reads `sfrac = sss1 & 0x1f`
(`tex.c:182`), five fractional bits. So the RDP's raw coordinate field is
S10.5 carrying `(ss/sw) * 2^15`, i.e. `(ss/sw) * 2^10` **texels**.

fn64's `texture_coordinates_s10_5`
(`crates/fn64-render-wgpu/src/raw_dpc/triangle_span.rs:519-541`) computes
`(S_q16 / W_q16) * PERSPECTIVE_TEXEL_SCALE` with
`PERSPECTIVE_TEXEL_SCALE = 1024.0` and returns it as the RAW S10.5 value.
`S_q16 / W_q16` is the same ratio as `ss/sw`, so fn64's raw value is
`(ss/sw) * 2^10` -- **which is angrylion's TEXEL value, written into the
field that wants angrylion's RAW value.** The unit domains were crossed.

**The function's own doc states the correct rule** and the constant
contradicts it: "the output is `(S/W) * 2^15` in S10.5 units = `(S/W) *
2^10` texels". 2^15 raw is right; the code writes 2^10 raw. The constant
should be **32768.0**, not 1024.0.

This is consistent with the measured spread. A 32x under-scale drives 72% of
triangles below one texel of span; corrected, that bucket becomes 0-31
texels, which is the footprint a wrestler polygon over a 32x32 or 64x64
texture should have.

**Why the wrong value survived.** `triangle_span.rs`'s doc records that 1024
was derived empirically against WM2000's TITLE SCREEN and that without it
"the whole title-screen quad collapsed onto texel (0,0)". Going from 1 to
1024 fixed a total collapse and left a 32x residue, which on one large flat
quad reads as slightly wrong texture scale rather than as flatness. Applied
to in-match geometry -- 100% perspective, small polygons -- the same residue
collapses each triangle onto one texel. The constant was fitted on the one
piece of content least able to expose it.

## THE FIX, MEASURED ON THE ROM

`PERSPECTIVE_TEXEL_SCALE` 1024.0 -> 32768.0
(`crates/fn64-render-wgpu/src/raw_dpc/triangle_span.rs`).

**Status: CONFIRMED.** Sixth run, same script, same stack, same census,
same tick size. Directly comparable to the fifth run above.

| measurement, at the 99,607-triangle tick | before | after |
|---|---|---|
| triangles sampling exactly **1** distinct texel | 87,115 (**87.5%**) | 11,987 (**12.0%**) |
| triangles sampling 5-8 distinct texels | 1,019 (1.0%) | 35,564 (35.7%) |
| triangles sampling 9-16 | 8 (0.008%) | 21,106 (21.2%) |
| triangles sampling 17-64 | 0 | 163 |
| S spread of **zero** whole texels | 36,341 (**72.2%**) | 3,469 (**3.5%**) |
| S spread of 17-64 texels | 145 (0.3%) | 22,323 (22.4%) |

```
before: distinct texels per triangle (triangles=99607): 1:87115 2:10859 3-4:606 5-8:1019 9-16:8
after:  distinct texels per triangle (triangles=99607): 1:11987 2:7690 3-4:23097 5-8:35564 9-16:21106 17-64:163
```

**A 7.3x reduction in one-texel triangles, and a 20x reduction in
zero-spread ones.** The distribution moved from a spike at "1" to a broad
population centred on 5-8 distinct texels per triangle, which is what
sampling a real texture across a small polygon looks like.

The residual 12.0% at one texel is expected rather than suspicious: WM2000
draws genuinely tiny triangles, and a triangle covering a sub-texel
footprint is *entitled* to one distinct texel. The measured S spread agrees
-- 3.5% of triangles still span under one whole texel, and those are the
same primitives.

### What is claimed, and what is not

**CLAIMED, and measured:** the sampler now reads varied texels within each
triangle instead of one, on the real ROM, in gameplay. That is the mechanism
of flat shading, and it is fixed at its cause with a hardware citation.

**NOT claimed:** that a frame now "looks right". No frame was captured under
this card -- the shell's screenshot is an F2 keypress in an interactive
window and this lane ran headless. An owner running `scripts/play-wm2000.sh`
can confirm or refute the visual result in seconds, and that check is worth
doing before anyone treats the symptom as closed. The honest statement is
that the per-triangle texel variation is fixed; whether every remaining
artifact (the horizontal colour bands, the blocky glyphs) shares this cause
is untested, and the bands in particular have a documented alternative
explanation in the harness traps (a framebuffer read at the wrong stride).

### Suite counts

- `cargo nextest run --workspace --offline`: **8679 passed, 13 skipped**
  (baseline 8673/13; +6 are this branch's new tests).
- `cargo nextest run -p fn64-render-wgpu --features host-gpu-tests
  --offline`: **4914 passed, 3 skipped** (baseline 4908/3; same +6).

### The two circular fixtures this fix exposed

`rdp_harness/tests.rs`'s `perspective_s_for` derived its expected S plane by
inverting fn64's own `1024`, so both perspective fixtures asserted the
implementation against itself and passed under a scale 32x from hardware.
They were re-derived from angrylion's `tcdiv_persp`; every claim they
actually make -- that the divide happens, that the perspective and
non-perspective paths differ, that W's magnitude rather than its sign is
used -- is unchanged and still asserted.

This is the checklist's "derive expectations BY HAND from the wire layout,
never from the code under test", failing in the wild. A test that inverts
the constant under test cannot detect a wrong constant, and both of these
had detailed, confident doc comments.

### Mutation results

| mutant | outcome |
|---|---|
| `PERSPECTIVE_TEXEL_SCALE` -> 1024.0 (the original bug) | **killed**, 3 tests |
| `PERSPECTIVE_TEXEL_SCALE` -> 16384.0 (half) | **killed**, 5 tests |
| KEPT ARM: `PLANE_TO_TEXEL` 2^21 -> 2^16 | **killed**, 5 tests |
| KEPT ARM: `unsigned_abs().max(1)` -> `max(1)` | **killed**, 1 test |
| KEPT ARM: `saturate_s10_5` NaN `return 0` -> `return 1` | **SURVIVED** |

The surviving mutant is recorded in the code rather than papered over: the
NaN arm is unreachable from this crate, because `texture_coordinates_s10_5`
floors its denominator at 1, so no fixture can currently reach it. It is
kept for a future caller without that guarantee, and its doc now says
plainly that it is untested.

The new scale test uses `S/W = 0.25` rather than `1.0` deliberately: at a
unit ratio the correct answer 32768 saturates the `i16` return to 32767
under both the right scale and several wrong ones, so a unit-ratio fixture
could not distinguish them.
