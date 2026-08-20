# WM2000 flat-shading: the combiner census

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
