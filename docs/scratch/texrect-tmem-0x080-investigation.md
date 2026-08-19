# WM2000 texrect TMEM byte 0x080 abort — investigation log

## The abort (measured by the controller, integration branch 80df40ac, real ROM)

```
crates/fn64-abi/src/task_dispatch/rsp_commit.rs:1202
execute_raw_dpc: render-wgpu/raw-dpc-execute backend error: texture rectangle
texel fetch failed at pixel (1, 15): physical TMEM texel byte 0x080 is invalid
```

Raised at `crates/fn64-render-wgpu/src/targets/texrect.rs:677` (`TexrectExecutionError::Sample`).
Reached after 280 VI swaps.

## Reading the base tree first (no code changes yet)

Facts established by reading, before any measurement of my own:

1. The prior-art fix (f2c52822, in the base) is in `production.rs`, not in the
   texrect path's own file. It added `WgpuBackend::tiles_before_last_plan`,
   snapshotted in `plan_raw_dpc` BEFORE `RdpState::apply(&delta)` and consumed
   by `execute_raw_dpc`.

2. That snapshot seeds `PlanCollector`'s in-order walk. The texrect path reads
   its tile from `collector.plan.triangle_neutral_tiles[triangle_index]`
   (`production.rs:3769`), which is pushed from `self.current_tiles` at each
   triangle's own stream position (`production.rs:1109`). So the texrect path
   consumes the SAME already-repaired walk the triangle path does — it is
   downstream of f2c52822, not a second unrepaired copy of it.

3. The texrect's tile INDEX is read from the texrect's own wire word 1 bits
   26:24 (`production.rs:1040-1046`), not defaulted to 0.

4. `texrect.rs` already carries a *previous* fix for a TMEM validity abort on
   this same path: `first_row_parity` is derived from
   `tile.size().low_t().integer() & 1` rather than a frozen `Even`. Its own
   commentary names the production abort byte `0x04c` and WM2000's
   `low_t.integer() == 47`. Two pinned tests in `tmem/read.rs`
   (`wm2000_texrect_pixel_sixty_three_reproduces_the_production_invalid_byte`,
   `wm2000_texrect_reads_only_loaded_bytes_under_the_writers_own_row_parity`)
   cover that repair.

## Consequence for the briefed first hypothesis

The briefed hypothesis was "the texrect path has the same wrong-tile-binding
defect as the triangle path". Points 2 and 3 above are evidence AGAINST a
straight repeat: the texrect path reads the repaired walk and reads the real
tile index. Not yet refuted — the binding could still be wrong for a different
reason — but the specific f2c52822 mechanism is already covered here.

Note also that the new failure is at pixel (1, 15) and byte 0x080, whereas the
`0x04c` parity abort was at pixel (63, 0). This is a DIFFERENT pixel and a
DIFFERENT byte, i.e. a genuinely new failure downstream of the parity repair,
not the same one resurfacing.

## Status

Measurement in flight: reproducing on the real ROM in my own worktree
(`recompile_rom` had to be built there first — the harness reads
`$FN64/target/release/recompile_rom`).

## Reproduced in my own worktree (run 2, `/private/tmp/tr2.log`)

Identical to the controller's measurement, so the abort is real and stable:

```
crates/fn64-abi/src/task_dispatch/rsp_commit.rs:1202
execute_raw_dpc: render-wgpu/raw-dpc-execute backend error: texture rectangle
texel fetch failed at pixel (1, 15): physical TMEM texel byte 0x080 is invalid
vi_swaps=280
```

(Run 1 was void: the harness reads `$FN64/target/release/recompile_rom` and
that binary did not exist in a fresh worktree. Built it, then re-ran.)

## Harness gotcha worth recording

`run-rs-lane.sh` ends in `exec env ROM=... FN64_ABSENT_N64DD=1 ... ./wm2000-boot`,
an `env` invocation with an explicit variable list. Any diagnostic env var you
export outside the harness is therefore DROPPED before the binary starts. A
first instrumented run produced no output for exactly this reason. Gate
temporary instrumentation on a marker FILE, not an env var, or edit the
harness.

## MEASURED: the failing texrect's own state (run 4, `/private/tmp/tr4.log`)

Instrumentation dumped at the abort site (marker-file gated):

```
FN64DUMP fail pixel=(1,15) s=10272 t=7392 parity=Odd
  fmt=Rgba px=Bits16 line_words=17 tmem_word=0 palette=0
  mask_s=0 shift_s=0 mask_t=0 shift_t=0
  low_s=1276 low_t=860 high_s=1536 high_t=956
  draw=(l=320,t=216,w=64,h=23)
  err=Read(InvalidTexelByte { address: 128 })
```

### Hand-replay from the wire layout (not from the code under test)

`shift_s == shift_t == 0`, so `relative = (coord - low * 8) >> 5`:

- `s`: `(10272 - 1276 * 8) >> 5 = (10272 - 10208) >> 5 = 64 >> 5 = 2`
- `t`: `(7392 - 860 * 8) >> 5 = (7392 - 6880) >> 5 = 512 >> 5 = 16`

`mask == 0` on both axes forces the clamp arm; `dim_s = 1536/4 - 1276/4 + 1 = 66`,
`dim_t = 956/4 - 860/4 + 1 = 25`. Both 2 and 16 are in range, so
`column = 2`, `row = 16`.

16bpp linear address: `tmem*8 + row*line_words*8 + column*2`
= `0 + 16 * 17 * 8 + 2 * 2` = `2176 + 4` = **0x884**.

`low_t.integer() = 860 >> 2 = 215`, odd, so `first_is_odd = true`;
`row = 16` is even, so `odd_row_exchange = true ^ false = true` → XOR 4.

Now the two candidate scopes:

| scope | mask | masked | after XOR4 |
|---|---|---|---|
| `FullTmem` | `0x0fff` | `0x884` | **`0x880`** |
| `LowHalf` | `0x07ff` | `0x084` | **`0x080`** |

The production abort names **`0x080`**. So the read took the **LowHalf**
scope. That is the measurement, derived by hand and matching to the byte.

### The TMEM validity bitmap says the low-half masking is what breaks it

Dumped alongside, all 4096 bytes:

- `0x884` (what `FullTmem` would have read) is **VALID** — the load wrote it.
- `0x080` (the `LowHalf` alias) is **INVALID**.
- Low half: 1988 / 2048 valid. High half: 2004 / 2048 valid. TMEM is broadly
  loaded across BOTH halves; this is not a sparsely-loaded tile.
- The 26 invalid runs are all exactly 4 bytes long and regularly spaced
  (0x080, 0x10c, 0x190, 0x21c, ...), i.e. the XOR4 partners of each loaded
  row's tail — the ordinary undefined padding a wider load leaves.

So the byte genuinely was never loaded, AND the byte the read should have
addressed genuinely WAS loaded. The guard is right; the address is wrong.

### Why the scope came out LowHalf

`AddressScope::of` returns `LowHalf` for `ReadKind::Indexed{..}` (RT64's
`or(isRgba32, usesTlut)`). This tile is `fmt=Rgba px=Bits16`, NOT RGBA32 and
NOT ColorIndex — so it can only have reached the indexed arm via `preflight`,
whose first arm is:

```rust
if lut_mode == TextureLutMode::Disabled && tile.format() != ImageFormat::ColorIndex {
    ... return Ok(ReadKind::Direct);
}
... return Ok(ReadKind::Indexed { palette });
```

i.e. an ENABLED TLUT sends *any* format down the indexed path, including a
plain RGBA16 tile, and `resolve_indexed_texel`'s `Bits16` arm then treats the
texel's high byte as a palette index.

## Hypothesis status

The briefed wrong-tile-binding hypothesis is **REFUTED for this abort**. The
tile's own geometry is self-consistent and its addressed byte 0x884 is loaded.
The defect is the address SCOPE, which follows from `lut_mode`, not from which
tile is bound.

Next question, and the one that decides the fix: is `lut_mode` genuinely
enabled at this texrect's stream position (in which case the RDP really would
index an RGBA16 tile through the palette, and the bug is upstream in what sets
`G_SETOTHERMODE` / which snapshot the texrect reads), or is the texrect reading
a `TextureLutMode` from the wrong stream position?

## The reader is FAITHFUL — checked against RT64, not assumed

`/Users/jer/Code/no-mercy-recompiled/third_party/rt64/src/shaders/TextureDecoder.hlsli:162-163`:

```hlsl
// Determine the TMEM address mask. When using RGBA32 or TLUT, each
// sample only addresses half of TMEM.
const uint addressMask =
    select_uint(or(isRgba32, usesTlut), RDP_TMEM_MASK16, RDP_TMEM_MASK8);
```

and `:174` `if (usesTlut) { ... }` branches on the TLUT flag BEFORE any format
dispatch, with the non-4b palette index taken as `pixelValue0` — i.e. RT64
does index a plain RGBA16 tile through the palette when TLUT is on, and does
confine that read to half of TMEM.

fn64's CPU reader (`AddressScope::of` + `preflight`) and its GPU shader
(`tmem_sample.wgsl:532`, `tmem_sample_texel` branching on `lut_mode` before
format) both implement exactly this. So NEITHER sampler is the defect, and
neither the `InvalidTexelByte` guard nor the low-half mask should be touched.

The question therefore reduces to: is `TextureLutMode::Rgba16` the right value
AT THIS TEXRECT'S OWN STREAM POSITION?

## MEASURED: the other-mode word at the abort (run 5, `/private/tmp/tr5.log`)

```
FN64DUMP othermode high=0x0008acef low=0x005041c8 lut=Rgba16 cycle=OneCycle
```

Hand-decoded from the `G_SETOTHERMODE_H` wire layout:

| field | bits | value |
|---|---|---|
| `G_MDSFT_TEXTFILT` | 13:12 | 2 (Bilinear) |
| **`G_MDSFT_TEXTLUT`** | **15:14** | **2 = `G_TT_RGBA16`** |
| `G_MDSFT_TEXTPERSP` | 19 | 1 |
| `G_MDSFT_CYCLETYPE` | 21:20 | 0 (OneCycle) |

So `TEXTLUT` really is set in the register the executor read. The decode is
right. The remaining possibility is that the executor read the register from
the WRONG STREAM POSITION.

## The prime suspect: `other_mode` was left out of f2c52822's repair

`execute_raw_dpc` (`production.rs:2125-2143`) seeds the executor's walk with:

```rust
self.rdp_state.other_mode(),        // <-- POST-fold
...
self.tiles_before_last_plan          // <-- PRE-fold (f2c52822's repair)
    .unwrap_or_else(|| durable_neutral_tiles(&self.rdp_state)),
```

`plan_raw_dpc` folds the whole packet's `RdpStateDelta` into `rdp_state`
BEFORE `execute_raw_dpc` runs. f2c52822 repaired that time-travel for the tile
registers only, and its own doc says so explicitly:

> Only the *tile* registers need this. Every other seeded register
> (`other_mode`, `combine`, the constant colors, `color_image`) is also folded
> early, and the same reasoning applies to them -- but they are not what this
> repair measured, and widening the snapshot to registers no measurement
> implicates would be a change with no evidence behind it.

This abort is a candidate for exactly the measurement that was missing. If the
packet carries a `SetOtherMode` that turns TLUT ON, and this texrect stands
BEFORE it, the texrect would be seeded with the packet's final TLUT-enabled
mode instead of its carried-in TLUT-disabled one — the identical class of
defect, one register over.

Measurement in flight to confirm or refute.

## PROVEN: the same time-travel defect, one register over (run 6, `/private/tmp/tr6.log`)

The failing texrect is `idx=0 tri=0 cmd=6` — the FIRST texrect of its packet.
Its packet's own fold, dumped at `plan_raw_dpc` immediately around
`rdp_state.apply(&delta)`:

```
FN64DUMP plan othermode_high pre=Some("0x00000cef") post=Some("0x0008acef")
FN64DUMP texrect idx=0 tri=0 cmd=6 seeded_other_high=0x0008acef n_tris=6
```

Hand-decoded, `G_MDSFT_TEXTLUT` = bits 15:14:

| | high word | TEXTLUT | meaning |
|---|---|---|---|
| carried in (pre-fold) | `0x00000cef` | 0 | **`G_TT_NONE` — TLUT OFF** |
| packet-final (post-fold) | `0x0008acef` | 2 | `G_TT_RGBA16` — TLUT ON |

The executor was seeded with the post-fold word, so the texrect at command
index 6 ran under a `SetOtherMode` its own packet issued LATER. Under the
carried-in mode the read is `ReadKind::Direct` at `AddressScope::FullTmem`:

- `0x884 & 0x0fff = 0x884`, XOR4 → `0x880`, and **`0x884`/`0x880` are measured
  VALID in the bitmap.**

Under the time-travelled mode it is `ReadKind::Indexed` at `LowHalf`:

- `0x884 & 0x07ff = 0x084`, XOR4 → `0x080`, **measured INVALID** — the abort.

That is the whole chain, and every link is measured or hand-derived:
wrong stream position → wrong TLUT bit → wrong `ReadKind` → wrong address
scope → an address the load never wrote → the guard correctly refuses.

**Class: the SAME defect as f2c52822 (time-travelled register state from the
plan/execute fold), on the register that commit explicitly declined to widen
to for want of a measurement. This is that measurement.**

The briefed wrong-tile-binding hypothesis is refuted; the tile was fine. The
guard, the low-half mask, and both samplers are all correct and untouched.

### The fix

Extend f2c52822's pre-delta snapshot to `other_mode`, exactly as it was done
for the tiles: snapshot in `plan_raw_dpc` before the fold, consume in
`execute_raw_dpc`. Narrow, and now evidence-backed.
