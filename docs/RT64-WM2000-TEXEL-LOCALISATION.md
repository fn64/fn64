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

---

# Session wrap-up: what landed, what is still wrong, what to do next

Written at hand-off. Every claim is **CONFIRMED** (measured this session) or
**HYPOTHESIS**.

## The headline: the XOR4 fix was real, cited, and NOT sufficient

**CONFIRMED.** The odd-row XOR4 defect above is a genuine defect with a
hardware citation, it is fixed, and both suites are green with it
(8683 workspace / 4916 host-gpu). **Textures are still noise.**

I ran the ROM myself on this branch (`docs/tools/wm2000-match-run.sh --frames`,
2M step budget) and looked at the result. `docs/frames/wm2000-after-xor4-fix-swap5192.png`
is committed. What it shows, described honestly:

- **Geometry is correct.** Two wrestler silhouettes are clearly discernible,
  correctly shaped and correctly placed, with yellow hair blocks in the right
  positions. The mat and ring region has real structure.
- **Texel values are wrong.** Every surface is dense magenta / green / black
  per-pixel speckle. It is noise, not imagery -- the same signature
  `RT64-WM2000-TEXTURE-STATE.md` recorded, unchanged in character.
- Zero panics and zero backend errors across 5,160 dumped frames to swap 5,021.
  The sampler is not erroring; it is returning wrong values.

**This is now the second cited, measured, insufficient fix in a row** -- after
`PERSPECTIVE_TEXEL_SCALE`. Both were real. Neither was the whole story. The
honest reading is that the noise has more than one contributing cause, and each
fix removes one without crossing the visibility threshold.

## What is RULED OUT, with evidence

These are the most valuable results here, because each closes a branch.

1. **TMEM address arithmetic is correct.** CONFIRMED by a standalone C probe
   implementing angrylion's `fetch_texel` addressing and fn64's
   `linear_byte_address` side by side over 614,400 combinations per pixel
   size. They agree exactly for every `t < 256` at 4/8/16-bit. Section 2 above.
2. **The byte-lane mapping is correct**, same probe.
3. **A silent fallback colour is not the cause.** CONFIRMED by reading the live
   path: `targets/raw_triangle.rs:495` propagates every `PointSampleError` into
   `TexrectExecutionError::Sample` and `?`-aborts the whole packet. There is no
   default texel. Combined with the ROM's zero backend errors, every sampled
   byte was a byte the load actually wrote.
4. **The tile binding is not frozen to tile 0.** CONFIRMED: `production.rs:4188`
   reads the triangle's own wire tile field and refuses an unbound tile by name.
5. **First-row parity is not frozen.** CONFIRMED: derived per tile at
   `raw_triangle.rs:392`, and now irrelevant anyway after the XOR4 fix.

## Two divergences found and NOT fixed

Both are real, both are narrow, and neither is likely to be the noise cause.

- **The missing `t & 0xff` row mask.** angrylion masks the row
  (`tile->line * (t & 0xff)`, `tmem.c:65`); fn64 does not. CONFIRMED
  divergence, but it only bites at `t >= 256`, which needs `mask_t` 0 or >= 9
  AND a row index that large. HYPOTHESIS that WM2000 does not reach it.
- **The un-truncated `shift` 11..=15 arm.** angrylion re-truncates to 16 bits
  after the left shift (`coord <<= (16 - shifter); coord = SIGN16(coord)`,
  `tcoord.c:90-100`); fn64 widens to `i64` and does not. CONFIRMED divergence
  with worked examples in section 3. HYPOTHESIS that WM2000's tiles do not use
  the 11..=15 shift encoding. **This is cheap to settle and nobody has:** dump
  the distinct `(shift_s, shift_t)` pairs WM2000 actually binds.

## The parity corpus extension: roughly 60% done

**What EXISTS and WORKS** (committed, `crates/fn64-render-conformance/src/bin/fn64-render-conformance-parity-runner.rs`):

- `OTHER_MODES_ONE_CYCLE_TEXTURED` -- a one-cycle, non-perspective, TLUT-off,
  POINT-sampled, no-AA/no-dither other-modes word, hand-derived field by field
  from angrylion's `rdp_set_other_modes` (`rdp.c:623-660`).
- `SET_COMBINE_TEXEL0` -- `(Zero - Zero) * Zero + Texel0` in both pipes and
  **both cycles**, hand-packed from `rdp_set_combine`
  (`combiner.c:522-539`) and its input tables.
- `TEXTURE_TEXELS` -- eight RGBA16 texels chosen so no two coincide and no one
  is a byte-swap of another, so a wrong row, a swapped bank and a wrong lane
  each give a different visible answer.
- Wire helpers `set_texture_image`, `set_tile`, `set_tile_size`, `load_tile`,
  `texture_rectangle`, and the list builder `one_textured_rect`.
- `seeded()` now stages the texture source through `RdramViewMut::write_u16`,
  so the guest byte-lane mapping is applied once (a `copy_from_slice` here
  would have reported a runner defect as a texture defect).
- Two cases: `textured-rect-point-sampled` and
  `textured-rect-second-row-only` (the second deliberately reads TMEM row 1,
  the row that carries the exchange -- the first case's row 0 cannot see it).
- `authoritative_cases_use_the_no_coverage_other_modes_word` retargeted from a
  literal-word equality to the three PROPERTIES the partition actually needs
  (dither selectors both 3, and AA_EN / ALPHA_CVG_SEL / CVG_TIMES_ALPHA all
  clear), checked against angrylion's own bit positions. **This was the single
  structural blocker** -- the old test made the corpus incapable of holding any
  non-fill case, because a textured draw cannot run in fill cycle.
- **All 18 corpus guard tests pass**, and the runner builds and runs.

**What does NOT work yet** -- the remaining ~40%:

- **wgpu REFUSES both textured cases.** CONFIRMED, measured:
  ```
  execute_texture_rectangle requires resident_bytes for already-resident
  target ColorTargetKey { ... }; treating a resident candidate as if it had
  no prior content would silently discard everything outside the rectangle
  ```
  This is a legitimate guard, not a bug to widen. The texrect path needs the
  target's prior bytes and the runner does not supply them. The fill lane hit
  the same class of thing and solved it by seeding a partial fill from the
  guest's own framebuffer; `ConformanceReplay` already carries `guest_rdram`,
  so the fix is plumbing, not new semantics. **Do not weaken the guard.**
- **The combiner fix is UNVERIFIED.** The last complete corpus run still showed
  the reference lane refusing with "G_TEXRECT combiner cycle 1 selects
  COMBINED before a first-cycle result exists". I derived
  `SET_COMBINE_TEXEL0`'s both-cycles form to fix exactly that, it compiles and
  the guard tests pass, but the run that would have confirmed it was cut off.
  **Re-run the corpus first thing.**
- **RT64 completes both cases but does not match the key**
  (`rt64_matches_key: false`). Not yet diagnosed. It may be my key, my display
  list, or a real finding -- unknown, and it must not be assumed to be a
  finding until the key is re-derived independently.
- No raw-triangle textured case (both cases are texture rectangles), no CI/TLUT
  case, no non-trivial `line`, and no captured-from-ROM case.

## Dead ends, so nobody pays for them twice

- **Making LoadBlock/LoadTile latch the tile's SL/TL/SH/TH.** I implemented
  this in `production.rs` and reverted it. Hardware genuinely does it
  (`rdp_load_block` `tex.c:907-917`), but it is NOT the fix here, because the
  T origin cancels out of the exchange on both sides anyway (section 4). It
  may still be worth doing for its own sake; it is not this defect.
- **Making the rdp_harness stage a multi-row LoadBlock with a nonzero DXT.**
  Reverted. With `line == words_per_row`, LoadBlock's destination is
  `word + advance * line`, so a nonzero DXT strides and leaves gaps -- rows
  cannot be both advanced and contiguous. That is real RDP behaviour, not a
  bug. Use LoadTile when a fixture wants stated rows.

## The next concrete step

**Re-run the parity corpus and resolve the two textured cases.** Precisely:

```sh
export FN64_RT64_DIR=/Users/jer/Code/no-mercy-recompiled/third_party/rt64
export CARGO_TARGET_DIR=/private/tmp/fn64-parity-target
cargo build -p fn64-render-conformance --features parity-runner \
  --bin fn64-render-conformance-parity-runner
"$CARGO_TARGET_DIR/debug/fn64-render-conformance-parity-runner" 2>/dev/null \
  | python3 -c 'import json,sys; d=json.load(sys.stdin); [print(r["case"], r["verdict"], r["rt64_matches_key"], r["wgpu_matches_key"], str(r.get("wgpu"))[:200]) for r in d["rows"] if "textured" in r["case"]]'
```

Then, in order:

1. Confirm the reference lane no longer refuses the combiner. If it still does,
   `SET_COMBINE_TEXEL0` is wrong -- re-derive it from `combiner.c:522-539`,
   not from any fn64 code.
2. Supply the texrect path its `resident_bytes` in
   `wgpu_bytes()` (parity-runner ~line 459), the same way the fill lane seeded
   a partial fill from `guest_rdram`. The guard is correct; feed it.
3. With both backends completing, compare against the key. The expected pixel
   at target `(0, 0)` is `TEXTURE_TEXELS[0]` = `0xf801`, and at `(0, 1)` is
   `TEXTURE_TEXELS[4]` = `0x8421`. **If wgpu returns `TEXTURE_TEXELS[2]`
   (`0x003f`) at `(0, 1)` that is a 4-byte bank error**; if it returns
   `TEXTURE_TEXELS[0]` the row stride is wrong; if it returns a byte-swap of
   the right value the lane mapping is wrong. Each is a different, named
   diagnosis -- which is the entire point of extending the corpus.

That comparison is the thing this session was trying to reach and did not.
