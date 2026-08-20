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

## The odd-row XOR4 exchange: writer and reader must agree

The RDP interleaves TMEM rows by XOR-ing the address by 4 bytes on odd rows.
angrylion does this on BOTH sides using the **tile-relative** row:

- Write (`tex.c:487` `loading_pipeline`): `tc_pipeline_load` applies
  `TRELATIVE(sst1, tile->tl)` (`tcoord.c:998-999`), making `sst` tile-relative,
  then `dswap = sst & 1` (`tex.c:583`).
- Read (`tmem.c:63` `fetch_texel`): `taddr ^= ((t & 1) ? ... )` on the equally
  tile-relative `t`. **`fetch_texel` never reads `tile->tl`** -- confirmed by
  grep; `tl` does not appear in the function.

So on hardware the exchange term is `row & 1` and there is no `tl` term
anywhere. What matters for correctness is only that the two sides agree.

### CONFIRMED: fn64's LoadTile is self-consistent

fn64's writer (`tmem/types.rs`, `TmemLoadKind::Tile` arm) uses
`(bounds.low_t().integer() + row) & 1`; its reader (`tmem/read.rs`,
`odd_row_exchange`) uses `first_is_odd ^ (row & 1)` with
`first_is_odd = low_t.integer() & 1`. Since
`(a + b) & 1 == (a & 1) ^ (b & 1)`, these are the same function. Enumerated
over `low_t` 0..4 x `row` 0..4: **writer and reader agree in all 16 cases.**

fn64 therefore places LoadTile bytes at different absolute addresses than
angrylion whenever `low_t` is odd, but reads them back through the same
displacement, so the texel values are unaffected. **Not the noise cause.**

### CONFIRMED DIVERGENCE: fn64's LoadBlock writer and reader disagree

The `TmemLoadKind::Block` arm uses a DIFFERENT parity source:

```rust
(u64::from(source_t.raw()) + advance) & 1 != 0
```

`source_t.raw()`, not `low_t.integer()`. The reader has only one rule and
always uses `low_t.integer() & 1`. So for a LoadBlock the two sides agree only
when `source_t.raw() & 1 == low_t.integer() & 1`.

Two independent reasons that equality is not guaranteed:

1. They are **different fields** -- the load's own T origin versus the tile
   descriptor's T origin.
2. They are in **different units** -- `.raw()` is S10.5 fixed point while
   `.integer()` is `raw >> 2`, so even for the same underlying coordinate the
   parity bit read is a different bit.

Enumerated over `source_t` 0..4 x `low_t` 0..3 x `advance` 0..3: **24 of 36
combinations disagree.** On a disagreeing row every texel is fetched from the
wrong 4-byte lane of its 64-bit word.

**Signature match:** wrong lane = wrong bytes = wrong colour, at correct
coordinates, on correctly shaped and lit geometry. That is exactly the
observed "noise, not imagery".

**HYPOTHESIS** (not yet confirmed) that this is THE cause for WM2000: it
requires WM2000's textured surfaces to be loaded with `LoadBlock` rather than
`LoadTile`, and requires the parity mismatch to actually occur for its tiles.
Both are measurable and are the next step.

## CONFIRMED: hardware's exchange term is the tile-relative row, and nothing else

The earlier note above guessed that the load command ought to latch its
SL/TL/SH/TH into the tile (which it does -- `rdp_load_block` `tex.c:907-917`,
`tile_tlut_common_cs_decoder` `:939-970`). That is true but is NOT the fix,
because on hardware the T origin **cancels out of the exchange entirely**.

`rdp_load_block` seeds the edgewalker's span T with `tl << 3`
(`lewdata[5] = ((sl << 3) << 16) | (tl << 3)`, `tex.c:929`). `loading_pipeline`
then takes `st = t >> 16` and hands it to `tc_pipeline_load`, which applies
`TRELATIVE(sst1, tile->tl)` -- subtracting the very `tl << 3` the span was
seeded with (`tcoord.c:998-999`). The write parity `dswap = sst & 1`
(`tex.c:583`) is therefore taken on a **tile-relative** row that starts at zero
for every load, whatever `tl` is.

The read side is the same: `fetch_texel` uses `(t & 1)` on the equally
tile-relative row and never reads `tile->tl` at all.

**So the hardware rule, on both sides and for every load kind, is exactly
`row & 1`.** There is no `tl`, `low_t` or `source_t` term anywhere.

### What fn64 has instead

| site | fn64's term | hardware |
|---|---|---|
| LoadTile writer | `(low_t.integer() + row) & 1` | `row & 1` |
| LoadBlock writer | `(source_t.raw() + advance) & 1` | `row & 1` |
| reader (both) | `(low_t.integer() & 1) ^ (row & 1)` | `row & 1` |

Enumerated over `low_t` 0..8 x `source_t` 0..8 x `row` 0..8 (512 cases):

- Each of the three sites differs from hardware in **256 / 512** cases.
- **LoadTile writer vs reader: 0 / 512 disagree.** The spurious `low_t` term
  is present on both sides and cancels, so LoadTile texels come back correct
  despite sitting at non-hardware absolute addresses.
- **LoadBlock writer vs reader: 256 / 512 disagree.** The writer's term is
  `source_t.raw()` and the reader's is `low_t.integer()` -- a different field
  in a different unit -- so nothing cancels.

**That is the defect.** On a disagreeing LoadBlock row every texel is fetched
from the wrong 4-byte half of its 64-bit word: wrong colour, at correct
coordinates, on correctly shaped and lit geometry.

### The fix

Remove the spurious origin term from all three sites so every one reads
`row & 1`, as angrylion does. This simultaneously:

- makes the LoadBlock writer and reader agree (the defect), and
- puts fn64's absolute TMEM addresses on hardware's own layout, which the
  LoadTile pair currently only emulates up to a cancelling displacement.
