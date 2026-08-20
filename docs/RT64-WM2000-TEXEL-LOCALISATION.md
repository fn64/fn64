# Localising the wrong texel value

The defect bounded by [`RT64-WM2000-TEXTURE-STATE.md`](RT64-WM2000-TEXTURE-STATE.md):
coordinates are right, the fetch at those coordinates returns wrong values, so
surfaces render as noise rather than imagery.

Every claim is marked **CONFIRMED** (measured this session) or **HYPOTHESIS**.

The authority throughout is angrylion (`/private/tmp/angrylion-probe`,
`src/core/n64video/rdp/`), read by hand and re-implemented in a standalone C
probe. Expectations are derived from angrylion's source, never from fn64's.

## The hardware pipeline, as angrylion states it

`texture_pipeline_cycle` (`tex.c:168-200`) is, in order:

1. `tcshift_cycle` (`tcoord.c:83`) -- per-axis `shift` on the RAW S10.5 value.
2. `TRELATIVE(x, sl)` = `x - (sl << 3)` (`n64video.c:21`) -- subtract the tile
   origin, in S10.5 units.
3. `sfrac = sss1 & 0x1f` -- five fractional bits taken HERE, before clamping.
4. `tcclamp_cycle` (`tcoord.c:128`) -- `*S = locs >> 5` to integer texels.
5. `tcmask_coupled` -- mask/mirror.

Then `fetch_texel` (`tmem.c:63`) addresses TMEM.

## CONFIRMED: TMEM address computation is NOT the defect

`fetch_texel`'s RGBA16 arm (`tmem.c:113-129`) is:

```c
uint32_t tbase = tile->line * (t & 0xff) + tile->tmem;
taddr = (tbase << 2) + s;
taddr ^= ((t & 1) ? WORD_XOR_DWORD_SWAP : WORD_ADDR_XOR);
c = tc16[taddr & 0x7ff];
```

with the true-hardware (big-endian) macro values `WORD_ADDR_XOR = 0`,
`WORD_XOR_DWORD_SWAP = 2`, `BYTE_ADDR_XOR = 0`, `BYTE_XOR_DWORD_SWAP = 4`
(`src/core/common.h:6-21`). The LSB_FIRST arm of those macros is host
byte-order compensation for angrylion's own swapped TMEM array, NOT hardware
semantics; reading it as hardware inverts the byte lanes.

fn64's `linear_byte_address` (`crates/fn64-render-wgpu/src/tmem/read.rs`)
computes `tmem*8 + row*line*8 + column*bytes_per_texel`, masks, and XORs the
BYTE address by 4 on odd rows. A standalone C probe implementing both
formulas and sweeping `line` 1..=16, `tmem` 0..512 step 64, `t` 0..300,
`s` 0..16 -- 614,400 combinations per pixel size -- finds:

| size | mismatches | condition |
|---|---|---|
| 16-bit | 45,056 / 614,400 | only `t >= 256` |
| 8-bit | 45,056 / 614,400 | only `t >= 256` |
| 4-bit | 45,056 / 614,400 | only `t >= 256` |

**For every `t < 256`, fn64 and angrylion agree exactly, at all three pixel
sizes.** The odd-row `^4` byte exchange, the 64-bit-word `line` stride, the
4-bit nibble halving and the address masks are all correct.

So **the byte-lane mapping and the TMEM address computation are refuted as the
cause of the noise.** Two of the four suspects in the handoff's list are gone.

### The one real address gap, and why it is not this defect

angrylion masks the row: `tile->line * (t & 0xff)`. fn64 does not. **CONFIRMED**
as a divergence; **HYPOTHESIS** that it is not the noise cause, on the grounds
that reaching `t >= 256` requires a tile whose `mask_t` is 0 or >= 9, and a row
index that large, which is not the common case for the wrestler/mat surfaces.
Worth fixing on its own merits, with its own fixture.

## CONFIRMED: the `shift` 11..=15 arm diverges

angrylion (`tcoord.c:90-100`), both arms:

```c
if (shifter < 11) { coord = SIGN16(coord); coord >>= shifter; }
else              { coord <<= (16 - shifter); coord = SIGN16(coord); }
```

`SIGN16(x)` is `((int16_t)(x))` (`n64video.c:15`), so the high arm
**re-truncates to 16 bits after the left shift**. fn64's
`relative_axis_coordinate` (`crates/fn64-render-wgpu/src/tmem/sample.rs`) does
`raw * (1_i64 << (16 - shift))` on a widened `i64` with no truncation.

Measured with a C probe over both implementations:

| raw | shift | angrylion | fn64 |
|---|---|---|---|
| 32767 | 11 | -32 | 1048544 |
| 32767 | 15 | -2 | 65534 |
| -32768 | 11 | 0 | -1048576 |
| 4096 | 13 | -32768 | 32768 |

**HYPOTHESIS** that this is not the noise cause either: it requires a tile with
`shift_s`/`shift_t` in 11..=15, which is the "negative shift" encoding and is
uncommon. Needs a census of WM2000's actual tile descriptors to settle.
