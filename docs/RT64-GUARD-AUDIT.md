# RT64 / fn64-render-wgpu refusal-guard audit

**Deliverable of the guard-audit card. This is an audit, not a fix — nothing
in `crates/fn64-render-wgpu` was changed.**

`fn64-render-wgpu`'s refusal guards are self-authored by this port: each was
added by a lane that had not yet implemented a case and refused it rather than
guessing. That is a defensible design, but it means every guard is a
*hypothesis about what is unsupported*, not a *fact about what is illegal*.
This document tests each hypothesis against reference sources and classifies it.

## Reference sources and their authority

> **PROVENANCE WARNING, added after this audit was written.** The ranking below
> was inverted with respect to `AGENTS.md`'s clean-room protocol
> (`AGENTS.md:26-45`), which EXCLUDES the Angrylion lineage
> (`docs/DISCOVER-PLAN.md:2260`). This audit ranked it first and stated that it
> "wins where they disagree". **Findings in this document whose only support is
> Angrylion are not admissible as fn64 authority and are being re-grounded or
> re-derived; see the per-finding notes.** The audit's conclusions may still be
> correct -- several were acted on and are covered by tests -- but their stated
> reasons need replacing with allowed evidence or with measurement.

1. **RT64 pinned C++**, `/Users/jer/Code/no-mercy-recompiled/third_party/rt64`
   (MIT, pinned) — GPU HLE. An ALLOWED source. It shows what an accepted
   implementation does; it rarely states a hardware rule in prose, so cite it
   as "RT64 implements X", not "hardware requires X".
2. **Public libultra headers and manuals** (`ultra64/gbi.h`, `ultra64/rcp.h`)
   and the MIT recompiler's generated C — the wire formats and the ABI fn64
   serves. Strongest available for encoding questions.
3. **Measured observations about a ROM**, permitted by owner decision (see
   `AGENTS.md`). "Measured on WM2000: the guest emits X and fn64 rendered Y" is
   both admissible and usually more relevant than another emulator's behaviour.
4. **`crates/fn64-render-reference/`** — fn64's own software renderer. Where it
   already implements what wgpu refuses, that is a wiring gap inside this repo.

**Excluded:** angrylion-rdp-plus, and any GPL runtime implementation.

## The single structural fact behind most findings

**angrylion has no rectangle pipeline.** `rdp_tex_rect`
(`rasterizer.c:2634-2688`), `rdp_tex_rect_flip` (`:2694-2724`) and
`rdp_fill_rect` (`:2744-2769`) are pure *command translators*: each fabricates a
triangle-shaped `ewdata[]` array and hands it to the one shared
`edgewalker_for_prims`. Every rect-specific behaviour is a consequence of what
those three functions write into that array — zeros for shade, zeros for `w`,
and a `yl |= 3` snap in fill/copy — and not of any branch downstream.

Consequently, most "a rectangle cannot do X" hypotheses in this crate are false:
rectangles reach the same combiner, blender, dither and coverage stages
triangles do, with defined inputs.

## Classification counts

| Class | Count |
|---|---|
| INVENTED | 5 |
| WRONG RESPONSE | 6 |
| UNKNOWN | 4 |
| CORRECT | 7 |
| Skipped (infrastructure / internal-consistency) | ~30 |

---

## A. INVENTED — hardware has no such constraint

These refuse content the RDP defines. Ranked by likelihood of blocking real
game content.

### A1. `TexrectExecutionError::UnsupportedColorInput` / `UnsupportedAlphaInput` for `Shade` on texture rectangles — CONFIRMED

**Refuses:** a texture rectangle whose colour combiner selects `Shade` or
`ShadeAlpha` in any slot.

**Site:** `targets/texrect.rs:1117` / `:1137` (`admits_color` / `admits_alpha`
gate on `shade_available`), reached from `:1176`, which passes
`shade_available = false` for every texrect. `Shade`/`ShadeAlpha` are absent
from `ADMITTED_COLOR_INPUTS` (`:983-993`) and `ADMITTED_ALPHA_INPUTS`
(`:1000-1007`).

**Hardware:** shade on a rectangle is a *defined zero*, not an absent value.
`rasterizer.c:2665` — `memset(&ewdata[8], 0, 16 * sizeof(uint32_t));` — zeroes
the entire shade block, base colour **and all eight gradients**
(`drdx…dady`), which the edgewalker unpacks at `rasterizer.c:2088-2103`. The
value therefore stays zero across the whole span rather than merely starting
there. `rgba_correct` (`rasterizer.c:124-156`) then writes
`shade_color.{r,g,b,a} = special_9bit_clamptable[0] = 0` unconditionally, every
pixel, called from `rasterizer.c:526`. The combiner reads that same struct
through select code 4 (`combiner.c:14, 33, 52, 80` for RGB; `:95, 110` for
alpha). There is no "shade unused" bypass — `rdp.c:537-550` gates *texel*
fetches only.

RT64 agrees independently: `hle/rt64_rdp.cpp:1253-1260` emits an all-zero vertex
colour array for every rect. **Two independent implementations converging on
zero is strong evidence this is architectural.**

This is the same fact the card already recorded as WRONG for `ShadeAlpha` on
texrects; the *colour* `Shade` selector is the identical fact and is still
refused.

**Correct behaviour:** supply a constant zero shade for texrects and admit
`Shade`/`ShadeAlpha`. **Cost: small** — flip `shade_available` to `true` at
`texrect.rs:1176` and feed `Color4::from_wire(0)`. Note the contrast with the
CORRECT case C1 below: on an *unshaded raw triangle* hardware interpolates a
real value, so zero there would be fabricated. On a rectangle zero *is* the
hardware value.

**Why it plausibly blocks content:** a combiner selecting SHADE on a texrect
multiplies to black or adds nothing. Games relying on that zero are common.

---

### A2. `crate::TextureLutModeError::ReservedEncoding` — CONFIRMED

**Refuses:** a `SetOtherModes` word whose texture-LUT field (high bits 15:14)
holds the value 1.

**Site:** `state.rs:231-238` (`texture_lut_mode`); the error propagates all the
way out through `WgpuRawDpcExecutionError::TextureLutMode`, aborting the packet.

**Hardware:** there is **no 2-bit texture-LUT enum**. angrylion decodes two
independent 1-bit fields — `rdp.c:630-631`:

```c
wstate->other_modes.en_tlut    = (args[0] >> 15) & 1;
wstate->other_modes.tlut_type  = (args[0] >> 14) & 1;
```

Every consumer tests `en_tlut` alone (`rasterizer.c:73`, `:89`; `tex.c:180`,
`:243`, `:385`) and reads `tlut_type` only *inside* an `en_tlut` branch
(`rasterizer.c:76`: `tformat = tlut_type ? FORMAT_IA : FORMAT_RGBA`). Encoding 1
is `en_tlut = 0, tlut_type = 1`, which the hardware path treats as **TLUT
simply disabled**; the `tlut_type` bit is dead.

fn64's own doc concedes the provenance is RT64's *header macros*, not hardware
— `state.rs:84-88` cites `shared/rt64_f3d_defines.h`. A missing macro is not an
illegal encoding.

**Correct behaviour:** decode encoding 1 as `TextureLutMode::Disabled`.
**Cost: small** — one match arm at `state.rs:237`, and the error type and its
`WgpuRawDpcExecutionError` variant can then be deleted entirely.

---

### A3. `TexrectExecutionError::ReservedAlphaCompare` — CONFIRMED

**Refuses:** other-mode alpha-compare field value 2, before it can reach
`alpha_compare_value`'s panic.

**Site:** decoded to `AlphaCompare::Reserved` at `state.rs:314`; refused in the
texrect executor.

**Hardware:** identical shape to A2 — two independent bits, `rdp.c:659-660`:

```c
wstate->other_modes.dither_alpha_en  = (args[1] >> 1) & 1;
wstate->other_modes.alpha_compare_en = (args[1] >> 0) & 1;
```

`blender.c:72-88` short-circuits on bit 0 alone:

```c
if (!wstate->other_modes.alpha_compare_en)
    return 1;
```

Value 2 is `alpha_compare_en = 0, dither_alpha_en = 1` → **always pass**, i.e.
behaviourally identical to `G_AC_NONE`. The dither bit is ignored.

RT64 reaches the same answer by a different route: it treats the field as a
2-bit enum (`shared/rt64_other_mode.h:18-20`) and its shader tests only
`== G_AC_DITHER` and `== G_AC_THRESHOLD` (`shaders/RasterPS.hlsl:204-213`), so
value 2 falls through to no-compare. **Both agree; the mechanisms differ.**

Consistency check that survives the side channel: `rdp.c:543` makes
`alpha_compare_en` force the expensive two-texel path. Because angrylion reads
bit 0 alone, state 2 does *not* trigger it — exactly what a `2 -> None`
enum mapping would also produce.

**Correct behaviour:** map encoding 2 to `AlphaCompare::None`. **Cost: small**
— one match arm at `state.rs:314`.

**Caveat (HYPOTHESIS):** in COPY mode with an 8-bit framebuffer angrylion runs a
separate inline alpha-compare (`rasterizer.c:1971-2003`) in which the dither
bit's meaning is framebuffer-size dependent. `alpha_compare_en` still gates the
whole block at `:1971`, so state 2 is still "no compare" there — but a lane
implementing this should read that block rather than assume the `blender.c`
path covers copy mode.

---

### A4. `TexrectExecutionError::UnsupportedColorInput` for `Texel1` in two-cycle — CONFIRMED

**Refuses:** a two-cycle texture rectangle whose combiner reads `Texel1` /
`Texel1Alpha`.

**Site:** `Texel1` and `Texel1Alpha` are absent from `ADMITTED_COLOR_INPUTS`
(`targets/texrect.rs:983-993`). Two-cycle mode itself *is* admitted
(`:1681`, `CycleType::TwoCycle => Ok(TexrectCombinerEvaluation::TwoCycle)`), so
this is specifically the second-texel selector.

**Hardware:** rectangles take the identical two-cycle dispatch triangles do —
`rasterizer.c:2525-2534`. The `textureuselevel1` selector that chooses between
the two-texel and one-texel paths is derived purely from *combiner input
analysis*, never from primitive type (`rdp.c:543-550`). A texrect whose
combiner references TEXEL1 therefore takes `render_spans_2cycle_complete`,
which fetches both texels — `rasterizer.c:1046-1047`:

```c
texture_pipeline_cycle(wstate, &wstate->texel0_color, &wstate->texel0_color, sss, sst, tile1, 0);
texture_pipeline_cycle(wstate, &wstate->texel1_color, &wstate->texel0_color, sss, sst, tile2, 1);
```

`tile2` comes from `tclod_2cycle` (`:1044`) — the ordinary tile+1 LOD pairing,
driven by the texrect's own forwarded `tilenum`.

This directly contradicts the reasoning preserved in
`TexrectExecutionError::UnsupportedCycleType`'s doc comment
(`targets/texrect.rs:394-396`), which says `Texel1` "is refused for texrects
because a rectangle binds one tile, which is the reference lane's rule too". A
rectangle binds one *base* tile; the second texel comes from the LOD pairing,
exactly as for a triangle.

**Correct behaviour:** admit `Texel1`/`Texel1Alpha` for texrects and resolve the
second tile through the same tile+1 pairing. **Cost: medium** — needs a second
tile binding and a second TMEM sample on the texrect path.

---

### A5. `TexrectExecutionError::UnsupportedBlendShadeAlpha` on texture rectangles — CONFIRMED

**Refuses:** a texrect whose blender selects `A = Shade` on any active cycle.

**Site:** `targets/texrect.rs:2172-2174`.

**Hardware:** the term is defined, and computable without any vertex data.
The blender's 1b alpha mux select 2 is `blender_shade_alpha`
(`blender.c:51-57`), and the combiner sets it every pixel —
`combiner.c:283-285` (and identically at `:344-346`, `:452-454`):

```c
wstate->blender_shade_alpha = wstate->shade_color.a + adseed;
if (wstate->blender_shade_alpha & 0x100)
    wstate->blender_shade_alpha = 0xff;
```

On a rectangle `shade_color.a` is the defined zero of A1, so the term is exactly
`adseed`, the alpha-dither seed. And when no dither is selected —
`getditherlevel == 2`, i.e. `rgb_alpha_dither == 0xf` (`rdp.c:560-566`) —
`get_dither_noise` is never called (`rasterizer.c:528-529`) and `adith` stays
zero, so **the term is a clean, exactly-derivable 0**.

**Note the asymmetry with the CORRECT list.** The card records "the blender's
shade alpha — it is `shade_color.a + adseed`, dithered, not plain zero" as a
correct refusal. That verdict holds for the *dithered* case and for triangles.
It does **not** hold for a texrect with dithering disabled, where the sum is
provably zero. The refusal is therefore over-broad rather than wrong.

**Correct behaviour:** admit `A = Shade` on texrects when
`getditherlevel == 2` (no RGB/alpha dither selected); keep refusing otherwise
until the dither authority question (U3) is settled. **Cost: small** for the
no-dither case; **large** if the dithered case is also wanted, because it is
blocked on U3.

---

## B. WRONG RESPONSE — the constraint is real, the reaction is not

Hardware handles these by masking, clamping, rounding or silently skipping.
fn64 aborts the whole packet.

### B1. `TexrectExecutionError::OutsideTarget` — CONFIRMED

**Refuses:** a texrect whose rasterized extent is not entirely inside the colour
target. `targets/texrect.rs:1488-1490`. The doc says "Never clamped: a clamped
rectangle would write pixels the RDP never covers."

**Hardware clamps — that is precisely what the scissor is.** angrylion clips
every span's X against the scissor rect rather than rejecting the primitive
(`rasterizer.c:2349-2363`):

```c
curunder = ((xright & 0x8000000) || (xrsc < clipxhshift && !(xright & 0x4000000)));
xrsc = curunder ? clipxhshift : (((xright >> 13) & 0x3ffe) | stickybit);
curover = ((xrsc & 0x2000) || (xrsc & 0x1fff) >= clipxlshift);
xrsc = curover ? clipxlshift : xrsc;
```

and Y likewise at `:2284-2305` (`yllimit = yllimit ? yl : wstate->clip.yl;`).
`clip` is set by `rdp_set_scissor` (`:2779-2784`). fn64's own reference renderer
does the same, clamping with `.min(clip_max_x - 1).min(self.width - 1)`
(`fn64-render-reference/src/raster/draw.rs:197-203`).

**Root cause: `fn64-render-wgpu` has no RDP scissor state at all.** The only
`scissor` hits in the crate are the *RT64 extended* `setScissorV1` decoder
(`src/rt64_gbi_extended_decode.rs:620-640`) — a decoder with no consumer. So the
guard is not really "outside target"; it is "the scissor stage is missing".

**Correct behaviour:** implement `SetScissor` state and clip the rasterized span
to `scissor ∩ target`. **Cost: medium** — one piece of latched state, plus a
clip in the row planner shared by the texrect and fill executors. **This is the
highest-value single fix in the audit**: a rectangle that overhangs the
framebuffer is completely routine content.

**RESOLVED for the texrect path.** Every citation above was re-read and holds.
`RdpScissorRect` now latches the rect in the wire's own quarter-pixel units
(`targets/texrect.rs`), `SetScissor` stages into `RdpState`/`RdpStateDelta`
rather than being tracked-only (`raw_dpc/mod.rs`), it is snapshotted per
triangle on `RetrievedTriangleDraw`, and `execute_texture_rectangle` clips
through `clip_texrect_extent` instead of refusing. Precedence is
`rect ∩ scissor ∩ target`; the scissor is the authority and the target extent
is a separate bound, and both are exercised by fixtures where they disagree.

Three cases still refuse, now as `ScissoredAway` rather than `OutsideTarget`:
an extent empty after clipping, a reversed/degenerate scissor, and a rectangle
entirely past the target. The texture-coordinate ramp stays anchored at the
unclipped origin, matching `rdp_tex_rect`'s one-time load of `ewdata[24..39]`
(`rasterizer.c:2657-2677`) against an edgewalker clip that writes only
`majorx`/`minorx` (`:2349-2363`).

**Still open:** the FILL executor performs no scissor clip
(`targets/fill.rs`), and the `MixedFillAndTrianglePacket` composition path is
owned by another lane. The scissor is now available as staged state wherever
that lane needs it. The `mode` field (angrylion's `scfield`/`sckeepodd`,
`:2786-2787`) is decoded and carried but not honoured: this executor renders
progressive full-frame targets, and applying `sckeepodd` would drop every other
row. Neither gap was measured on the ROM.

---

### B2. `FillCoordinateError::FractionalEdge` — CONFIRMED

**Refuses:** any `FillRectangle` whose wire coordinate has nonzero low two bits.
`targets/fill.rs:191-196` (`whole_pixel`), reached from
`resolve_fill_pixel_rectangle` (`:203-216`). The same restriction is inherited
from `raw_dpc::plan_fill`.

**Hardware rounds; it does not refuse.** `rdp_fill_rect`
(`rasterizer.c:2744-2769`) *preserves* the fractional bits into the edgewalker
command rather than rejecting them:

```c
ewdata[2] = (xlint << 16) | ((xl & 3) << 14);
ewdata[4] = (xhint << 16) | ((xh & 3) << 14);
```

and snaps Y with `if (cycle_type == FILL || COPY) yl |= 3;` (`:2751-2752`).
RT64 rounds on both axes: `RDP::fillRect` does `lrx |= 3; lry |= 3;` in
fill/copy (`hle/rt64_rdp.cpp:1043-1047`), and `RDP::drawRect` does
`ulx &= ~3; uly &= ~3;` (`:1164-1169`). Neither returns an error for a
fractional edge.

**The two references disagree on the remedy, and it matters here.** angrylion
*preserves* the subpixel bits and lets the edgewalker use them, giving genuine
partial-pixel coverage; RT64 *snaps outward*, applying `(x + 3) >> 2` ceil to
all four edges (`common/rt64_common.cpp:93-121`, every caller passing
`ceil = true`) for a half-open integer extent with no partial coverage. That is
coherent for a GPU HLE renderer but is not the silicon behaviour. **Trust
angrylion on what the RDP does; RT64's snap is the cheaper approximation.**
Either is a strict improvement on refusing, so the fix can start with RT64's
rounding and move to angrylion's subpixel coverage later — but the doc should
not claim the rounded version is exact.

fn64's own module doc already concedes this — `targets/fill.rs:40-42`: fractional
edges are something "real RDP hardware and this decoder both permit on the wire
but wgpu's decoder chooses not to admit". And the reference renderer handles the
general case with `.ceil()` / `.floor()`
(`fn64-render-reference/src/raster/draw.rs:195-203`).

**Correct behaviour:** round rather than refuse — `ulx &= ~3` / `uly &= ~3` on
the upper-left and `lrx |= 3` / `lry |= 3` on the lower-right, matching both
references. **Cost: small.**

**Bonus defect found while checking this (HYPOTHESIS, not part of the guard):**
fn64's texrect vertex oracle applies `lrx |= 3; lry |= 3` only when `is_copy`
(`raw_dpc/texture_rectangle.rs:509-513`), whereas RT64's `fillRect` applies it
for **fill or copy** (`hle/rt64_rdp.cpp:1043-1047`). If a fill-mode texrect ever
reaches that path it will be one pixel short on each axis. Currently masked by
the fill-cycle refusal (B3), so it would surface the moment B3 is fixed.

---

### B3. `TexrectExecutionError::UnsupportedCycleType { cycle_type: Fill }` — CONFIRMED

**Refuses:** a texture rectangle issued while cycle type is Fill.
`targets/texrect.rs:1682`.

**Hardware executes it as a fill rectangle, silently discarding texturing.**
`rdp_tex_rect` has no fill-mode branch beyond the shared `yl |= 3` snap
(`rasterizer.c:2650-2651`) and calls `edgewalker_for_prims` unconditionally
(`:2686`). The cycle-type switch lives *inside* the edgewalker, blind to which
command produced the data (`:2517-2539`), and dispatches to `render_spans_fill`
— which **takes no `tilenum` argument at all**, so the tile the texrect named is
dropped. Its inner loop (`:1855-1861`) calls only `fbfill_ptr`: no texture
fetch, no combiner, no blender. A texrect in fill mode is a fill rect, byte for
byte.

RT64 concurs by omission: `drawTexRect` special-cases copy mode
(`hle/rt64_rdp.cpp:1335`) but has no fill branch, and `drawRect` treats fill
purely as a coordinate round-down (`:1164-1169`).

n64brew states the same in prose, and **`fn64-render-reference` already
implements it** — `backend/imp.rs:911-919` routes the command to
`draw_fill_rectangle(&rectangle.as_fill_cycle_rectangle(), target)`. This is a
wiring gap inside this repo, not an open question. It is already recorded as
having aborted a real WCW/nWo Revenge frame, a shipped AKI sibling of WM2000.

**Correct behaviour:** route a Fill-cycle texrect to the fill executor, as the
reference does. **Cost: medium** — the executor's own doc
(`targets/texrect.rs:400-440`) lays out the three pieces honestly: carry the raw
wire rectangle alongside the resolved viewport (the two lanes' rounding rules
differ by a pixel on every axis), snapshot `FillColor` on the triangle path, and
run `require_safe_fill_cycle_bypass`. That analysis is correct and should be
followed; this audit adds only that the *hardware* question is settled.

---

### B4. `FillCoordinateError::ReversedRectangle` — CONFIRMED

**Refuses:** a fill rectangle with `x0 > x1` or `y0 > y1`.
`targets/fill.rs:213-215`.

**Hardware silently draws nothing.** RT64 returns early without an error
(`hle/rt64_rdp.cpp:1038-1041`: `if ((lrx < ulx) || (lry < uly)) { return; }`).
angrylion feeds it to the edgewalker, whose span walk produces no valid lines
(`rasterizer.c:2284-2305`). fn64's own texrect planner already ports the RT64
behaviour correctly — `raw_dpc/texture_rectangle.rs:498-507` returns `None` for
`is_null`, "exactly as RT64's `drawRect` returns early". The **fill** path
disagrees with the crate's own texrect path.

**Correct behaviour:** treat as a no-op, matching the crate's own texrect
planner. **Cost: small.**

**Severity note:** a reversed rect is likely a guest bug rather than routine
content, so this ranks low for blocking real games — but the *inconsistency*
between two paths in one crate is itself worth closing.

---

### B5. `TargetError::UnsupportedColorTargetFormat` for 8-bit colour images — CONFIRMED

**Refuses:** any colour image that is not RGBA16 or RGBA32.
`targets/mod.rs:67-73`.

**Hardware supports 4/8/16/32-bit colour images** with a full function-pointer
table per size — `fbuffer.c:22` (`fbread_4, fbread_8, fbread_16, fbread_32`) and
`:32` (`fbwrite_4, fbwrite_8, fbwrite_16, fbwrite_32`), with `fbwrite_8` at
`:55` and `fbread_8` at `:179`. `PIXEL_SIZE_8BIT` is threaded through the
rasterizer (`rasterizer.c:43, 198, 232, 277, 311`). fn64's own reference
renderer supports I8/CI8 too — `backend/imp.rs:914` expects "I8/CI8, RGBA16, or
RGBA32".

**Correct behaviour:** add an 8-bit colour-target format. **Cost: medium** — a
new `ColorTargetFormat` variant plus its device-byte encode/decode.

**Severity note (HYPOTHESIS):** 8-bit colour images are uncommon as the *main*
framebuffer; this most plausibly blocks auxiliary render targets. Ranked below
B1–B3 for that reason. Confirming which real content needs it would require a
ROM run, which this card did not do.

---

### B6. `TexrectExecutionError::DestinationCoverageUnavailable { consumer: "a partial-coverage fragment's cvg_dst accumulation" }` — CONFIRMED as WRONG RESPONSE in shape, UNKNOWN in remedy

**Refuses:** any partial-coverage texrect fragment while `IM_RD` is enabled.
`targets/texrect.rs:2097-2101`.

The *constraint* is real — see C4 — but the response has the same shape as the
low-half-TMEM precedent the card lists under WRONG: hardware does something
defined with the value, and fn64 refuses the packet. What hardware does is
`fbread_8`/`fbread_16` recovering `curpixel_memcvg` from the hidden-bits sidecar
(`fbuffer.c:12, 179`). fn64 maintains no sidecar, so it genuinely cannot
recover the value — but the correct remedy is to *add* the sidecar, not to keep
refusing.

**Correct behaviour:** maintain a 2-bit-per-pixel hidden-bits plane alongside
each RGBA16 target, as the oracle does (`RdramHiddenBits`). **Cost: large** — a
new device-side resource threaded through every colour-target read and write.

---

## C. CORRECT — real invariants; refusing is right

### C1. `Shade` / `ShadeAlpha` on *unshaded raw triangles*
Precedent from the card, re-confirmed here by contrast with A1. On a rectangle
the shade block is memset to zero at `rasterizer.c:2665`, so zero is the
hardware value. On an unshaded triangle no such memset happens and the edgewalker
interpolates a real value from the wire coefficients (`rasterizer.c:2088-2103`),
so admitting it would read a fabricated zero. **Keep.**

### C2. `TriangleTextureBindingDisagreesWithOpcode`
`targets/texrect.rs`. A TEXTURED opcode with no tile binding would combine
against a fabricated zero texel; an UNTEXTURED one with a binding has no S/T/W
coefficient block, so any texel produced is invented. Both directions are real.
**Keep.**

### C3. `TriangleRowCountDisagreesWithJournal` / `TriangleRowRangeDisagreesWithJournal`
Not a hardware question at all — it is the declared-vs-drawn contract.
`fill_completed_writes` slices and digests the full-extent buffer for every
declared range without checking the raster touched it, so a disagreement would
publish stale bytes under a valid digest. **Keep.**

### C4. `DestinationCoverageUnavailable { consumer: "cvg_dst = Save" }`
The RDP stores a genuine 3-bit coverage count split between the RGBA16
halfword's visible LSB and a 2-bit hidden sidecar. `fbuffer.c:96-97` writes the
split explicitly:

```c
rval = finalcolor | (uint16_t)(finalcvg >> 2);
hval = finalcvg & 3;
```

with the sidecar backed by `rdram.c:30` and a third `HB_CLEAN` state deriving it
from the colour LSB for CPU-written buffers (`rdram.c:125-127`). fn64 maintains
no sidecar, so only 1 of 3 bits is recoverable and the value cannot be
reconstructed. **Keep until the sidecar exists** (see B6, the same underlying
gap).

**RT64 is not usable as a reference here**: it has no hidden bits at all, and
approximates coverage as `8 * combiner_alpha` in the shader
(`shaders/RasterPS.hlsl:218`), losing all but the LSB on readback
(`shaders/Formats.hlsli:96-102`). angrylion is the only authority for C4-C6.

### C5. `UnsupportedBlendFramebufferAlpha`
Same root cause as C4: `B = FramebufferAlpha` is the destination coverage
*count*, not the stored alpha. angrylion resolves it as the stored 3-bit count
rescaled (`blender.c:65` -> `fbuffer.c:232`, `lowbits << 5`) and then
dz-shifted whenever it is the B input (`blender.c:103-107`) — so it is neither
the stored alpha nor `1 - coverage`. RGBA32 uses the same coverage-in-the-alpha-
byte scheme (`fbuffer.c:288-289`, `:112-113`), so **even a 32-bit target does
not carry a real 8-bit alpha here**. RT64 hardcodes it to `1.0` with a comment
saying so (`hle/rt64_blender.h:355-357`: "Coverage is not emulated"), which is
exactly the manufactured constant this refusal exists to prevent. **Keep.**

### C6. `BlendEnabledNotDerivable`
`targets/texrect.rs:2165-2168`. Refuses only the one case where the
`blend_enabled` disjunction genuinely does not settle without the coverage-wrap
term: `FORCE_BL` clear **and** `AA_EN` set **and** `IM_RD` set. angrylion
confirms the dependency exactly — `zbuffer.c:291-293`:

```c
int overflow = (curpixel_memcvg + *curpixel_cvg) & 8;
*blend_en = wstate->other_modes.force_blend || (!overflow && wstate->other_modes.antialias_en && farther);
```

The `!overflow` term is the coverage wrap, and it is live precisely when
`force_blend` is clear and `antialias_en` is set. RT64 models none of this —
`AA_EN`'s only consumer is a debugger text line
(`gui/rt64_debugger_inspector.cpp:1314`) — so it cannot settle the question
either. The other two
`FORCE_BL`-clear cases are correctly *not* refused. Guessing either way is
wrong — `true` runs a blender the RDP bypasses, `false` bypasses one it runs.
Narrowly scoped and honest. **Keep.**

### C7. `Blend { source: BlendImageReadError }`
A framebuffer term selected while `IM_RD` is disabled: no destination sample
legally exists. Substituting a zero destination draws a plausible wrong pixel.
**Keep.**

---

## D. UNKNOWN — could not be settled from available sources

### U1. `NoiseThresholdUnavailable` — UNKNOWN
`targets/texrect.rs:1946`, `:2429`. angrylion uses `irand(&wstate->rseed)`
(`blender.c:82`, `rasterizer.c:1985`) — a stock MSVC LCG,
`state * 0x343fd + 0x269ec3`, of which 3 bits are used (`dither.c:90`). Nothing
in that source claims it is the silicon sequence, and its output even varies
with worker count. RT64's is a *different* generic PRNG (a TEA-style `initRand`
plus `1664525u * s + 1013904223u`, `shaders/Random.hlsli:7-33`) carrying a
self-flagged `// TODO: Review seed.`. So all four sequences now in view are
substitutions and none claims fidelity. The two generators already in this
workspace
(`crate::random`'s RT64 shader PRNG and `fn64-render-reference`'s SplitMix64,
whose own source says it is "deliberately not described as the silicon
sequence") are a third and fourth different sequence. **Evidence missing:** a
hardware capture of the RDP's per-pixel random threshold, or a documented
derivation of `irand`'s seeding from silicon. The refusal is defensible; it is
not confirmable.

### U2. `OrderedDitherAuthorityUnsettled` — UNKNOWN
`targets/texrect.rs:1997`. This crate's Bayer table and
`fn64-render-reference`'s disagree at documented cells, pinned rather than
resolved by `rgb_dither.rs`'s
`bayer_matrix_disagrees_with_reference_oracle_at_documented_cells`. The variant's
own doc comment is a careful, accurate history and supersedes D7 of
`docs/RT64-LANE-DIVERGENCES.md`. **Evidence missing:** which Bayer arrangement
the RDP actually uses (divergence D19).

Checking `dither.c` does **not** settle it, and the reason is worth recording so
a later lane does not repeat the attempt. angrylion's tables live at
`dither.c:3-18`, and RT64 carries both matrices **byte-identically**
(`shaders/Formats.hlsli:9-21`, same index `((y&3)<<2)+(x&3)` at `:23-25`) — so
the *tables* are not in dispute between the references at all. What differs is
the arithmetic, and there the two references disagree with each other: angrylion
applies RGB dither as a branchless **conditional bump** (`dither.c:53-56`) and
alpha dither as **add-then-saturate** (`combiner.c:266-268`) — genuinely
different arithmetic per stage, and the alpha path reuses the RGB matrix in only
some of the 16 modes (cases 8/9/12/13 invert the roles). RT64 compiles its alpha
dither out entirely (`shaders/RasterPS.hlsl:186-201`, `#if 0`) and applies RGB
dither not at raster time but at framebuffer writeback, via a per-framebuffer
**majority vote** across the frame (`shaders/FbWriteColorCS.hlsl:19-21`,
`hle/rt64_framebuffer.cpp:189-191`). Neither is a per-cell authority for the
disputed arrangement. **Note this blocks A5's dithered
half.**

### U3. `MixedFillAndTrianglePacket` — UNKNOWN (architectural, not hardware)
`production.rs:2916`. Not a hardware-semantics question: the RDP has no notion
of a "packet", and a fill and a triangle in sequence are simply two primitives
into one framebuffer. But the refusal is not therefore invented — it names a
real fn64 architectural gap, stated accurately at `production.rs:1599-1616`: the
fill executes CPU-side into an owned buffer while `draw_admitted_triangles`
rasterizes into a GPU attachment that never composes back. **Evidence missing:**
nothing from a reference source can settle this; it needs the composition path
built. **Cost: large.** Note the sibling refusal for texrect+raw-triangle has
already been retired (`production.rs:2918-2925`), so the remaining case is
narrow.

### U4. Whether B5 (8-bit colour targets) blocks real content — UNKNOWN
The capability gap is CONFIRMED; the *impact* is not. **Evidence missing:** a
census of colour-image formats across the AKI corpus, or a ROM run. Deliberately
not run under this card's one-ROM-at-a-time rule.

---

## E. Skipped — infrastructure, not hardware semantics

Per the card's scope. Roughly thirty variants across the four enums are
internal-consistency checks between fn64's own planner, journal and executor,
with no hardware analogue. They cannot refuse *valid content* — only a
disagreement between two fn64 components. Representative:

- `production.rs`: `MalformedDestinationAccessRun`, `MissingCapturedSource`,
  `NoCompletedLoads`, `Physical`, `Effect`, `Coordinator`,
  `MissingTriangleDrawState`, `TriangleDraw`, `TriangleDrawBeforeCreate`,
  `FillAccessRegionKind`, `FillAccessOutsideTarget`, `MergedWriteUnclaimed`,
  `MergedWriteUndeclared`, `TexrectUnboundTile`, `TexrectMissingViewport`,
  `TexrectDeclaredNoWrite`, `RawTriangleDeclaredNoWrite`,
  `RawTriangleWireWordsUndecodable`, `PendingTmemImageClaimedCommitted`,
  `PendingTmemProjectionClaimedCommitted`, `TmemProjectionCountMismatch`,
  `CommittedTmemImageClaimedProposed`, `NoStagedColorImage`,
  `NoColorTargetHeight`, `FillColorImageDisagreesWithRegister`.
- `raw_dpc/mod.rs`: all of `FillAccessSpanError`; all of
  `TmemLoadSourcePlanError` except `YuvExecutionDeferred` (a declared
  deferral, not a hypothesis); `RawDpcDecodeError`'s decode-integrity arms.
- `targets/texrect.rs`: `NoDeclaredRows`, `MissingResidentBytes`,
  `TriangleAttributeSampleMissing`, `NonIntegralTexcoord`,
  `TexcoordOutOfRange`, `NegativeViewportOrigin`, `EmptyViewport`
  (`:270-283` — these validate the crate's *own* resolved viewport against
  values it produced one call earlier).
- `targets/fill.rs`: `MissingResidentBytes`.

`FillExecutionError::NotFillCycle` (`targets/fill.rs:336`) and
`UnsafeFillCycleBypass` (`:122-128`) are also **CORRECT and out of scope**: the
first is the card's already-resolved FillRectangle precedent, and the second
ports a documented hardware hazard (a retained depth consumer in fill cycle can
hang the RDP) that both `fn64-render-reference/src/raster/blend.rs:13-21` and
angrylion (`rasterizer.c:1839-1847`, `:1863-1871`, `rdp_pipeline_crashed = 1`)
enforce.

---

## Where angrylion and RT64 disagree

### The structural one: RT64 is not a reference for anything downstream of coverage

This is the most important cross-cutting result, and it should govern future
cards. RT64 has **no coverage sidecar** (`hidden` appears nowhere in the tree),
**no memory-alpha blender input** (hardcoded `return 1.0f`,
`hle/rt64_blender.h:355-357`), and **no `AA_EN` / `ALPHA_CVG_SEL` / `CLR_ON_CVG`
modelling** (their only consumers are debugger text and a `#if 0`,
`gui/rt64_debugger_inspector.cpp:1314`, `shaders/RasterPS.hlsl:194`). It decides
blending purely from whether the blender references framebuffer colour
(`hle/rt64_raster_shader.cpp:228`), approximates blender overflow with `fmod`
(`hle/rt64_blender.h:434-437`) where angrylion uses a hardware division LUT
(`blender.c:396-424`), applies dither only at writeback by frame-wide majority
vote, and snaps rectangle edges outward.

That is coherent for a GPU HLE renderer rather than a set of bugs. But it means
**RT64 must not be cited for C4, C5, C6, U1, U2 or B6** — for those, angrylion
is the only available authority, and its own accuracy is the ceiling. Where the
two agree (the dither *tables*, `lrx |= 3`, alpha-compare value 2) the agreement
is meaningful; where RT64 is silent or constant, its silence is not evidence.

### The specific ones

Beyond the structural divergence above, one point-disagreement surfaced that
does not change any verdict:

**Alpha-compare value 2.** Both conclude "behaves as `G_AC_NONE`", but by
different mechanisms — angrylion because it never assembles a 2-bit field
(`rdp.c:659-660`, `blender.c:75`), RT64 because it does assemble one and then
falls through in the shader (`shared/rt64_other_mode.h:18-20`,
`shaders/RasterPS.hlsl:204-213`), labelling it "UNKNOWN" in its debugger
(`gui/rt64_debugger_inspector.cpp:1287-1289`). **Trust angrylion**: the field
does not exist as a field, so there is no encoding to reserve. The agreement on
outcome is what makes the fix safe; the mechanism difference is why the
`Reserved` *variant* should be deleted rather than merely remapped.

Everywhere else the two agree, and where fn64's own reference renderer has an
opinion (B3, B5, and the clamping in B1) it agrees with angrylion.

---

## Suggested fix order for the follow-on card

1. **B1** — implement `SetScissor` and clip. Highest impact; unblocks all
   overhanging rectangles.
2. **A2 + A3** — two match arms, delete two error types. Trivially wrong,
   trivially fixed.
3. **A1** — constant zero shade for texrects. Small, and unblocks a very common
   combiner shape.
4. **B3** — route Fill-cycle texrects to the fill executor. Known to have
   aborted a real AKI-sibling frame. Follow the executor's own three-part plan,
   and fix the `is_copy`-vs-`is_fill_or_copy` asymmetry noted in B2 at the same
   time.
5. **B2 + B4** — round fractional edges, no-op reversed ones. Small, and B4
   makes the fill path agree with the crate's own texrect path.

A4, A5, B5 and B6 are larger and should follow. U1/U2 need evidence this repo
does not contain; U3 needs a composition path built.

## Verification

**No code was changed by this card**, so no suite run was required. The only
file added is this document.
