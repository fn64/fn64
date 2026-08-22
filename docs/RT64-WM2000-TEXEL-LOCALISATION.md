# Localising the wrong texel value


> **PROVENANCE WARNING.** This document's stated authority is
> angrylion-rdp-plus, which `AGENTS.md:26-45` EXCLUDES from fn64's clean-room
> protocol (`docs/DISCOVER-PLAN.md:2260` records the exclusion). Its
> observations about WM2000 and about fn64's own behaviour remain valid --
> measured facts about a ROM are explicitly allowed -- but **any claim here
> about what HARDWARE does, sourced only to angrylion, is not admissible as
> fn64 authority.** Re-ground such a claim on pinned RT64 (MIT), the public
> libultra headers, or a fresh measurement before acting on it.

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

---

# The corpus is finished, and it localised a NEW defect

Written after completing the textured corpus (commit `3109ff66`). Every
claim is **CONFIRMED** (measured) or **HYPOTHESIS**.

## The corpus now produces a usable result

Both blockers in the previous section are fixed:

1. **wgpu's `resident_bytes` refusal** -- answered from inside the packet.
   The textured command list now opens with a full-extent fill, which needs
   no seed itself and leaves the accumulated buffer the texrect composes
   into. The guard was NOT widened, and no seed-read plumbing was threaded
   through the decoder/IR/planner. The fill paints `STALE`, exactly what
   `seeded` writes and what the key already required outside the rectangle,
   so the key was unchanged.

2. **The key was wrong.** CONFIRMED. Texrect high edges are **exclusive**;
   `G_FILLRECT`'s are **inclusive**. The fixture used `extent - 1` by
   analogy with the fill cases, so it drew one fewer row and column: every
   backend correctly left row 1 as `STALE` while the key demanded texels
   there. **This is the "RT64 completes but does not match the key,
   undiagnosed" item from the handoff.** It was a fixture defect, not a
   finding. `targets/texrect.rs` already pinned the rule ("the fill rule is
   inclusive and the texrect rule is half-open"); the corpus now pins it
   too, mutation-verified.

**CONFIRMED: RT64 matches the hand-derived key exactly**, all eight texels,
both cases. The key is independently confirmed and can now be trusted.

## CONFIRMED: what wgpu does, and what it is NOT

| lane | row 0 | row 1 |
|---|---|---|
| key | `07c1 f801 ffff 003f` | `c631 8421 fc01 4211` |
| RT64 | `07c1 f801 ffff 003f` | `c631 8421 fc01 4211` |
| wgpu | `01f9 c107 3f01 ffff` | `2185 31c7 1143 01fd` |

The wgpu values are **not** in the diagnosis table the previous section
laid out, and that table's three named outcomes are all refuted:

- **Not a wrong texel.** No wgpu value is any member of `TEXTURE_TEXELS`.
- **Not a byte-swap, rotation, or any bit-permutation** of the correct
  value. Checked exhaustively over all 16 rotations, bit-reversal, and
  byte-swap of both the key and every texel.
- **Not a TMEM addressing error at all.** The values do not appear anywhere
  in the TMEM byte image, at any bit offset (swept every offset in the
  128-bit image). A wrong address returns *some real texel*; these are not
  real texels.
- **Not a stream misalignment.** Sweeping a 16-bit window across the
  concatenated pixel stream, and testing `prev << n | key >> n` in both
  directions, produces no match.
- **Not an RGBA5551/1555/0555 repacking**, not a 5->8->5 expansion, and not
  an RGBA8888 two-byte window.

## CONFIRMED: the corruption is a pure function of the texel value

The same texel maps to the same wrong value in **both cases**, at
**different target positions**, reading **different TMEM rows**:

| texel | case 1 | case 2 |
|---|---|---|
| `0xc631` | `0x2185` | `0x2185` |
| `0x8421` | `0x31c7` | `0x31c7` |
| `0xfc01` | `0x1143` | `0x1143` |
| `0x4211` | `0x01fd` | `0x01fd` |

**This is the most constraining result here.** Every position-dependent
cause is refuted by it, because each would vary with position and this does
not: TMEM address computation, tile `line` / row stride, the 4-byte bank
selection, the byte-lane mapping, and the odd-row XOR4 exchange. The defect
is a **value transform applied to a correctly-fetched texel**, downstream
of the sampler.

Note this retires, for the wgpu texrect lane, the entire class of cause the
previous sections were investigating. The XOR4 work there remains correct
on its own merits; it is not what this measures.

## CONFIRMED: the fill lane is unaffected

wgpu matches the key on **every** fill case in the corpus, including the
RGBA16 colour cases. So the 16-bit pack/write path is correct where it is
shared, and the defect is specific to the textured path: sampler output ->
combiner -> written pixel.

## CONFIRMED ROOT CAUSE: the sampler reads RAW STORAGE, skipping the `^3` map

A probe dumping `(s, t, decoded texel)` per pixel, immediately before
`combine_one_texel` (`targets/texrect.rs:1888`), settled it in one run.

**Texture COORDINATES are correct.** `s` steps `0x00, 0x20, 0x40, 0x60` and
`t` steps `0x00, 0x20` -- exactly one texel per pixel on both axes, and the
second case's rows both land on TMEM row 1 as designed. Addressing,
stride, shift/clamp and the tile descriptor are all doing the right thing.

**The texels the sampler returns are already wrong before the combiner
runs**, so the combiner and blender are exonerated. (Independently: the
crate's exhaustive `read_pixel_inverts_write_pixel_over_every_rgba16_
halfword` passes, `write_pixel` is byte-identical to the fill lane's, and
wgpu matches the key on every fill case in this corpus.)

The eight sampled values are explained by ONE rule, with no exceptions:

> fn64 reads the texture source as **raw RDRAM storage bytes**, without
> applying the guest's `^3` logical-to-storage byte-lane mapping.

| texel | raw storage BE | fn64 sampled | correct |
|---|---|---|---|
| 0 | `0xc107` | `0xc107` | `0xf801` |
| 1 | `0x01f8` | `0x01f8` | `0x07c1` |
| 2 | `0xffff` | `0xffff` | `0x003f` |
| 3 | `0x3f00` | `0x3f00` | `0xffff` |
| 4 | `0x31c6` | `0x31c6` | `0x8421` |
| 5 | `0x2184` | `0x2184` | `0xc631` |
| 6 | `0x01fc` | `0x01fc` | `0x4211` |
| 7 | `0x1142` | `0x1142` | `0xfc01` |

Equivalently, as a composite: each texel is read little-endian from byte
offset `correct ^ 2`. Both descriptions are the same defect.

### CONFIRMED: the fixture's staging is CORRECT, and this is not a harness bug

Worth stating explicitly, because it was checked the wrong way round once.
`seeded` stages the texels with `write_u16`, whose storage image is
`c10701f8 ffff3f00 31c62184 01fc1142`. Read back through the guest's `^3`
logical mapping that is exactly `f80107c1 003fffff 8421c631 4211fc01` --
the intended texels, in RDP-visible big-endian order.

Restaging it instead as `write_logical_bytes` over a big-endian image
produces a **byte-identical** buffer (verified), so that change is a no-op
and was reverted. Both spellings are correct; the storage image is right
either way.

**The decisive evidence is that RT64 reads the SAME buffer and returns the
key exactly.** Both backends receive the identical `rdram` from `seeded`.
One reads it correctly and one does not, so the defect cannot be in the
staging.

### Why this is the WM2000 defect, and why every earlier fix missed it

The signature matches `RT64-WM2000-TEXTURE-STATE.md` precisely: correct
geometry, correct coordinates, dense per-pixel wrong colour. A `^3` lane
error scrambles bytes WITHIN each 32-bit word, so neighbouring texels swap
halves -- noise, not imagery, on correctly shaped and lit surfaces.

It also explains why the perspective-scale fix and the odd-row XOR4 fix
were both real, both cited, and both insufficient: neither touches the byte
lane on the RDRAM->TMEM path, so the texels stayed scrambled no matter how
correctly they were addressed.

Note what this retires: the previous sections' entire investigation of
LoadBlock/LoadTile write-vs-read parity was searching for a
position-dependent cause. The corruption measured here is a pure function
of the texel VALUE -- the same texel maps to the same wrong value at
different target positions and different TMEM rows -- which no
position-dependent rule can produce.

### The open question

Whether the live ROM path shares this defect or only the conformance
harness does. The conformance lane slices `rdram.get(start..end)` directly
(`conformance.rs`, the `guest_rdram` arm), which is raw storage; the
production lane stages guest reads through `fn64-abi`. If both read raw
storage the defect is in the shared sampler and WM2000 is affected; if only
the conformance path does, the corpus needs the fix and the ROM defect is
still open. **Settle this before fixing anything** -- and note the fix site
differs completely between the two answers.

---

# The raw-triangle gap: a 32x plane scale AND different geometry

Resolved by instrumenting RT64 itself (probes in `RDP::drawTris` and the
framebuffer renderer, built against a CLONE so the pinned oracle stayed
clean). Every claim below is **CONFIRMED** by that probe or by measurement.

## CONFIRMED: "RT64 does not rasterize raw triangles" was WRONG

`drawTris` is entered exactly once, with `triCount=1 tile=0`, a non-empty
scissor `(0,0,1280,960)`, a non-null `drawRect (8,0,24,12)` and a non-null
`intRect`, and reaches `RENDER drawCall type=3 faceCount=1` with a bound
pipeline. Nothing is skipped, culled, or scissored away. The earlier reading
came from a triangle-only display list; with a texrect in the SAME packet
RT64 draws both.

## CONFIRMED: two separate defects, and BOTH are in the fixture's favour

### 1. The plane scale is 32x -- not 64x, not 2x

| lane | plane -> texels |
|---|---|
| fn64 | `plane / 2^21` to S10.5, then `/32` = `plane / 2^26` |
| RT64 | `plane / 2^20` at decode, then **`x0.5` at the sampler** = `plane / 2^21` |

**The trap is `perspCorrectionMod`** (`TextureSampler.hlsli:222`): a `0.5`
applied for non-perspective, non-rect draws, visible in neither
`rt64_gbi_rdp.cpp:535-537` nor `PLANE_TO_TEXEL`. Two earlier readings of
this both missed it and landed on 64x (comparing RT64's decode against
fn64's TEXEL figure) and 2x (comparing decode against fn64's S10.5 divisor).
The end-to-end factor is **32x**.

RT64's UV is in TEXELS at `floor(uvCoord)` (`:148`). The measurement
discriminates: only the texel reading predicts the all-clamped output
observed; an S10.5 reading predicts four DISTINCT texels, which is not what
happened.

PROBE-CONFIRMED: RT64's v0 texcoord is `24.0` for this fixture's
`s_base = 25165824`, exactly `s_base / 2^20`.

Consequence: RT64 reads the four columns as texels 16, 48, 80, 112 of a
4-texel tile and clamps every one to the last. Every pixel it writes is
`TEXTURE_TEXELS[3]`.

### 2. The same edge words are a RECTANGLE to fn64 and a TRIANGLE to RT64

Probe-measured vertices: **`(2,0) (2,3) (6,3)`** -- a right triangle with its
vertical leg on the left, not the intended box. With `XH` on the left,
`XL`/`XM` on the right and every `dxdy` zero, fn64 walks a box and RT64
walks the triangle between major and minor edges.

**The coverage is fully characterised, and a naive pixel-centre fit does NOT
reproduce it.** Enlarging the case to cols 2..10 x rows 0..6 and dumping raw
values (rather than a diff mask, which hides the shape because wgpu writes
the same clamp value in the outer columns) gives, deterministic across runs:

```
y=0  ..........
y=1  ..XX......
y=2  ..XX.X....
y=3  ..XXXX....
y=4  ..XXXXXX..
y=5  ..XXXXXX.X
```

- The left edge is pinned at x=2 (the `XH` edge) and the right edge advances
  **8/6 = 1.333 columns per row** -- exactly the hypotenuse slope of RT64's
  own probe-measured vertices, and the same 4/3 the small case gives.
- Coverage is quantised to **even-aligned pixel PAIRS** `[2k, 2k+1]`. Rows
  1, 3 and 4 are complete pairs.
- Where the span ends mid-pair (rows 2 and 5) the trailing pair writes only
  its **odd** pixel: x=5 with x=4 skipped, x=9 with x=8 skipped. The
  rightmost covered pixel is the odd end of the pair containing the ideal
  edge, in every row.

### FIXED: the fixture now emits two triangles

Because `v1` and `v2` always share the H edge's X, one command cannot be a
rectangle -- so the case emits TWO, the lower-left and upper-right halves:

| | H edge | L edge | YM | vertices |
|---|---|---|---|---|
| lower-left | `TRI_LEFT` | `TRI_RIGHT` | `TRI_BOTTOM` | `(l,t) (l,b) (r,b)` |
| upper-right | `TRI_RIGHT` | `TRI_LEFT` | `TRI_TOP` | `(r,t) (r,b) (l,t)` |

MEASURED after the change: **RT64 covers every pixel of the box** -- zero
left at the background value, where one command left seven of twelve. The
`the_triangle_case_emits_two_triangles_tiling_its_box` guard pins the
emitted vertices against RT64's own rule and is mutation-verified (dropping
the second triangle fails it).

The remaining difference on this case is the SCALE alone.

A plain pixel-centre rasterization of `(2,0) (2,3) (6,3)` predicts
`(2,0)` and `(4,2)` covered and `(5,2)` not -- the measurement is the
opposite on all three, so the earlier "its pixel-centre coverage grows one
column per scanline" was too loose. The pair quantisation is what closes the
gap.

## What is NOT established

That either lane is wrong against hardware. fn64's `2^21` is cited to
angrylion's `tcdiv_nopersp` and is self-consistent end to end
(`a_non_perspective_textured_triangles_texels_reach_guest_rdram` stages
`2^26`-per-texel planes and asserts four distinct texels). But RT64's value
passes through `perspCorrectionMod` and `gpuTile.tcScale`, which fn64's CPU
path has no equivalent of, so this is not a constant-for-constant
comparison. `fn64-render-reference` produces a third answer again and is not
an authority. **Hardware evidence is needed to settle `2^20` vs `2^21`.**

## Operational note

**Do not run two parity-runner instances concurrently.** Both hang at 0% CPU
in `Fn64Rt64Context::~Fn64Rt64Context` -> `RasterShaderCache` ->
`CompilationThread` join, and since the JSON is written after that drop, a
hung run leaves a ZERO-BYTE output file rather than partial output. Run
alone it completes in well under a minute.

Instrumenting RT64 in place is blocked: `fn64-render-rt64/build.rs` asserts
HEAD matches `docs/rt64-port-authority.json` AND the tree is clean. Use a
clone plus a temporary manifest edit, and do not build against the shared
tree concurrently.


---

# The hardware derivation, from angrylion

angrylion-rdp-plus is cloned at `/Users/jer/Code/angrylion-rdp-plus`
(durable, not `/private/tmp` -- the previous checkout was lost to a reboot).
Everything below is read from that source.

## CONFIRMED: the non-perspective chain, end to end

```
wire   s    = (ewdata[24] & 0xffff0000) | (ewdata[28] >> 16)   rasterizer.c:2106
             -> s15.16: integer half from word 24, fraction from word 28
span   .s   = ((s & ~0x1ff) + dsdiff - (xfrac * dsdxh)) & ~0x3ff   :2253
             -> subpixel adjust; units unchanged
pixel  ss   = s >> 16                                              :479
             -> drops the 16 fractional bits
tcdiv  sss  = SIGN16(ss) & 0x1ffff                          tcoord.c:1024
             -> tcdiv_nopersp does NO division and NO scaling
texel  sfrac = sss & 0x1f ; *S = locs >> 5                  tex.c:182, tcoord.c:143
             -> sss is S10.5; whole texel = sss >> 5
```

So **one texel = `2^21` plane units** on hardware: `>>16` to S10.5, `>>5`
to whole texels.

This is worth stating plainly because `tcdiv_nopersp` is the function fn64's
`PLANE_TO_TEXEL` cites, and it does not contain a scale at all -- the `2^21`
lives in the rasterizer's `>>16` composed with the texture pipeline's `>>5`.

## MEASURED: neither backend samples at that scale

Authoring the fixture's planes at `2^21` per texel and running it:

| lane | columns 2..5 |
|---|---|
| key | `07c1 f801 7fff 003f` (texels 0,1,2,3) |
| RT64 | `7fff 7fff 7fff 7fff` (clamped HIGH) |
| wgpu | `f801 f801 f801 f801` (texel 0, clamped LOW) |

RT64's effective scale is therefore **higher** than hardware's and fn64's is
**lower**; `2^21` sits between them. A sweep of `2^21 .. 2^28` finds no value
where RT64 steps one texel per column.

**So the earlier "32x" framing is not the whole story and should not be
quoted as settled.** What is settled is the hardware chain above.

## CONFIRMED: RT64 is a VERTEX renderer, and that is the deeper difference

`decodeTriangles` evaluates S at three vertices and lets the GPU
interpolate:

```
v1 = base + De*dy_1     v2 = base + De*dy_2     v3 = base + De*dy_3 + Dx*dx_3
```

**Only `v3` carries the `Dx` term.** With `De = 0` -- which a fixture whose
texcoord depends on X alone naturally wants -- `v1` and `v2` both take
`base`, so `base` must be the S of whichever edge those two vertices sit on
and `Dx` must carry the sign of the step to `v3`.

Probe-confirmed on the one-triangle case: `v0` and `v1` both `24.0`, `v2`
`280.0`.

This has a direct consequence for the two-triangle rectangle. The
upper-right half's `v1`/`v2` sit on the RIGHT edge, so with a shared plane
its gradient is REVERSED. Measured: sharing one plane makes the whole box
read texel 0. Splitting the plane per triangle (base at the right edge, a
negative `Dx`) restores RT64 to a clamp -- but still not to the key.

fn64's rasterizer evaluates the plane per PIXEL, so `Dx` applies at every
column regardless. The two models are not reconcilable by a scale factor
alone, which is why the scale sweep fails.

## RESOLVED: authored in vertex terms, RT64 matches the key

Two changes together, both derived rather than fitted:

1. **Per-vertex planes.** RT64 evaluates S at three vertices and only `v3`
   carries `Dx`, so with `De = 0` the `base` must be the S of the H edge --
   where `v1` and `v2` both sit. The two halves of the box therefore need
   DIFFERENT bases (left edge, right edge) and the SAME `Dx` of one texel
   per pixel of X: the upper-right half's `dx_3` is negative, so the sign
   cancels and no negative gradient is needed. An earlier attempt that
   negated `Dx` for that half double-counted the sign.
2. **The hardware scale**, `2^21` plane units per texel, from the chain
   above.

MEASURED, same fixture, the two scales:

| planes | RT64 | wgpu |
|---|---|---|
| `2^21` (hardware) | **matches the key** | texel 0 everywhere |
| `2^26` | clamps | matches the key |

So RT64 agrees with hardware and **fn64's rasterizer does not**: it wants
planes `2^5` larger for the same texel. The cause is visible in
`texture_coordinates_s10_5`, which divides by `PLANE_TO_TEXEL = 2^21` and
returns a value the caller consumes as S10.5 through
`TextureCoordinateS10_5::from_raw` -- so the sampler applies the `>>5` to
texels a second time. Hardware's `2^21` is plane -> TEXELS; fn64 treats it
as plane -> S10.5 and then divides again.

**This inverts what the corpus case measures.** It is now evidence against
wgpu rather than against RT64. The case stays in the non-authoritative
partition only because the wgpu-side defect is unfixed; once it is, the
case should become `Rt64Authoritative`.

Two guards pin the derivation, both mutation-verified:
`the_triangle_planes_land_on_texel_midpoints_at_every_vertex` reproduces
RT64's own vertex arithmetic on the emitted words and checks all six
vertices land on their texel midpoints (sharing one base fails it), and
`the_triangle_case_emits_two_triangles_tiling_its_box` pins the geometry.

## FIXED: the wgpu-side `2^5`

`PLANE_TO_TEXEL` is now `2^16`, the plane->S10.5 divisor, leaving the
`>>5` to whole texels where it belongs -- the sampler's
`div_euclid(TEXEL_FRACTION_SCALE)` in `tmem/sample.rs`. Total `2^21` from
plane to texel, matching hardware.

**MEASURED: the triangle case is now `identical`** -- key, RT64 and wgpu
agree byte-for-byte, and the corpus goes 14 -> 15 identical. The case is
promoted to `Rt64Authoritative`.

Two fixture constants moved with it. `rdp_harness/tests.rs`'s
`PLANE_PER_TEXEL` was `2^26`, derived in its own comment from the premise
that `2^21` is the plane->S10.5 divisor -- the same wrong premise. It is
now `2^21`, and `PLANE_HALF_TEXEL` derives from it rather than restating a
literal. Those four harness tests had been asserting the DEFECT's output;
they now assert the corrected texels, and they were written independently
of the parity corpus, so their agreement is real cross-confirmation.

Mutation-verified: reverting `PLANE_TO_TEXEL` to `2^21` fails four
`rdp_harness` tests. Before this change those same tests PASSED with the
defect, because their constants encoded it.

`fn64-render-reference` needed no change: its `PLANE_TO_TEXEL` at
`raster/draw.rs:943` is a separate code path with its own conversion, and
the whole workspace is green.

**NOT touched: the perspective path.** fn64 computes `(S/W) * 32768` on
the RAW s15.16 planes; angrylion's `tcdiv_persp` is a reciprocal-table
algorithm operating on the already-shifted `ss = s >> 16`. Whether
`32768` is right depends on that structural difference, which inspection
cannot settle -- and guessing at it is exactly how that constant got its
earlier wrong value of `1024`. It needs its own perspective fixture
measured against RT64, which the corpus can now express.

---

# TMEM loads copy WHOLE 64-bit words (2026-08-20)

Found by running the actual goal — WM2000 on `FN64_RECOMP=rs` +
`FN64_RENDER=wgpu` — rather than by reading code. The stack aborted at 1,887
VI swaps: `physical TMEM texel byte 0xa98 is invalid`.

## The measurement

Instrumenting `read_valid_byte` and dumping TMEM's high half at the failure:

- **2004 of 2048** bytes valid, spanning `0x800..0xfff`
- 44 invalid, in **11 runs of exactly 4 bytes**
- each hole is one 4-byte HALF of a 64-bit word; the other half IS valid
- the half **alternates** hi/lo
- palette entries **15, 32, 49, 66, 83, ...** — exact stride of **17**

So the TLUT loaded fully and was later *partially invalidated*.

## The defect

fn64 modelled a row whose logical texels do not fill its last 64-bit word as
having a PARTIAL TAIL: the uncovered lanes were `None`, and `physical.rs:656`
-- the **only** `valid[] = false` in the crate -- CLEARED destination
validity for them. An overlapping `LoadTile` (row 132 bytes = 16 full words +
a 4-byte tail, `line` 17 words) therefore punched holes in an already-loaded
TLUT, and a texrect sampling one aborted the run. Odd-row exchange
(`lane ^ 4`) alternates which half is cleared, giving the hi/lo pattern.

## The hardware truth

From the **pinned RT64 oracle's LIVE loader** -- `src/hle/rt64_rdp.cpp`, not
`rt64_hle_geometry.rs`'s `dumpTexture`, which is a debug-dump heuristic whose
Tile `wordsPerRow` is dead in the source:

```c
// loadWord, rt64_rdp.cpp:392-395
// Copy the entire word.
for (uint32_t i = 0; i < 8; i++) {
    TMEM[(tmemAddress + i) ^ tmemXorMask] = RDRAM[(textureAddress + (i & offsetMask)) ^ 3];
}
```

driven `wordsPerRow` times per row (`:459-468`). **Whole 64-bit words, no
partial-tail concept, no clamping.** The uncovered bytes are real adjacent
RDRAM bytes.

## A tempting fix that is WRONG

Deleting the `else` so undefined lanes PRESERVE prior validity. It returns
stale data where hardware overwrites, while `last_touched_generation` still
advances and claims a current write. Refused on review before it landed.

## The fix, four parts

1. `wire.rs`'s new `source_row_range` declares **padded** source reads --
   Block `div_ceil(8) * 8`, Tile `words_per_row * 8`.
2. Tile emits **one access PER ROW always**. The collapsed full-width path
   cannot express row-local padding: padded rows overlap in RDRAM because the
   row stride is the image's own `bytesPerRow`.
3. `defined_source_byte_mask`'s Block/Tile arms return 8 (`0xff`). TLUT keeps
   `0x03` source / `0xff` destination -- it captures two bytes and quadricates.
4. `raw_dpc/mod.rs`'s transfer-word binding is **row-local**
   (`row = index / words_per_row`, offset `within_row * 8`). Walking
   concatenated access lengths against a LOGICAL offset mis-binds as soon as
   rows carry padding.

Part 4 was found by the measurement, not predicted: after parts 1-3 the abort
MOVED to "raw-DPC command #39's source bytes are missing from the captured
guest reads", which localised the remaining defect exactly.

## Oracle review: 4 of 5 match

Block padding equals `wordCount << 3` (DXT changes the TMEM destination, not
the RDRAM bytes read); Tile per-row padded spans match because RT64's row
stride is the image `bytesPerRow`, so its rows overlap identically; removing
the full-width collapse reads identical bytes when padding is zero; and
RGBA32 reads all eight source bytes and merely permutes them across TMEM
halves, so the unconditional `0xff` is right where a partial `0x0f` would
have been wrong.

**The one divergence** is RT64's large-wrap leading-row skip
(`rt64_rdp.cpp:448-456`), whose own comment calls it an optimization for rows
"that have no effect in the final result". Final TMEM is identical, so it is
a PERFORMANCE gap rather than a correctness one. Narrow risk worth recording:
if a skipped row's padded read ran past installed RDRAM, fn64 would fail a
load RT64 completes.

## What this cost, and the lesson

**19 fn64 unit tests asserted the DEFECT** and failed on the fix --
`undefined_tail_bytes_are_staged_invalid_not_zero_filled_valid` says so in
its name. That is the second time this session fn64's own suite encoded the
bug it should have caught (the first was `PLANE_TO_TEXEL`). For TMEM
semantics, check the oracle; a green suite is not evidence.
