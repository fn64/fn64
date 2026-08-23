# Task 17 report: Z-buffer binding + depth test in the fn64 wgpu renderer

## Result

All 6 `gen-zbuffer-*` parity cases went from **wgpu-refused** to
**`wgpu_vs_angrylion_diff_pixels: 0`** (byte-identical to the bit-accurate
angrylion oracle, and to RT64). The `check_rt64_parity.py` gate **PASSES**
(exit 0). No regressions: `pass-all-match-hardware` rose 49 -> 55 (exactly the
six z cases); `wgpu-refused` fell 14 -> 8; `fn64-defect` (1) and
`shared-ported-bug` (7) unchanged (pre-existing loadblock/tmem items).

| case | before | after |
|---|---|---|
| gen-zbuffer-compare-disabled | wgpu-refused | 0-diff (pass) |
| gen-zbuffer-farther-loses | wgpu-refused | 0-diff (pass) |
| gen-zbuffer-nearer-wins | wgpu-refused | 0-diff (pass) |
| gen-zbuffer-source-sel-pixel-wins | wgpu-refused | 0-diff (pass) |
| gen-zbuffer-update-disabled | wgpu-refused | 0-diff (pass) |
| gen-zbuffer-setmaskimage-binds-z-image | wgpu-refused | 0-diff (pass) |

## Where the refusal was

Every case refused at the plan-probe decode with
`decoded opcode 0x3e ... is outside the M3.2 subset` -- the `_ =>` fallthrough
arm in `crates/fn64-render-wgpu/src/raw_dpc/mod.rs` (`UnsupportedCommand`).
Both wire opcodes the corpus uses mask to the same value: `SetZImage` (`0xfe`)
and `SetMaskImage` (`0x3e`) both `& 0x3f == 0x3e`, so a single missing dispatch
arm blocked all six.

## The binding

- `raw_dpc/mod.rs`: new `SET_Z_IMAGE = 0xfe & 0x3f` (== `0x3e`) constant and a
  decode arm that masks/range-checks `w1 & 0x00ffffff` exactly as
  `SetColorImage` does and produces a new `RawDpcCommandKind::SetZImage(ZImage)`.
  `state.rs` carries the small `ZImage` (address-only) type.
- `raw_dpc/production_adapter.rs`: `SetZImage` is **admitted, tracked-only** --
  deliberately not pushed to the neutral `RdpStateCommand` IR (which has no
  z-image variant, and adding one would ripple through every exhaustive match
  in `fn64-render-reference` for a value the depth path never consults). This
  is the same tracked-only precedent `SetScissor` already set. Admitting it is
  what turns the refusals into depth-tested draws.

## The depth test

- The primitive depth is threaded to each draw: `RetrievedTriangleDraw` gains
  `prim_depth: Option<PrimDepth>`, tracked in both draw-state collectors
  (`production.rs`'s `PlanCollector` and `raw_dpc/triangle_draw_data.rs`) from
  the already-neutral `SetPrimDepth` command, seeded from durable state.
- `production.rs::stage_color_commands` grows a **per-pixel depth accumulator**
  (`Vec<DepthCell>` = `(u32 working_z, u8 encoded_delta)`), seeded to `(0, 0)`
  -- the value a zeroed guest z-image decodes to -- allocated only when a draw
  in the packet requests `Z_CMP`/`Z_UPD`. It persists across the schedule
  exactly like the colour accumulator, so a later draw sees the depth an
  earlier one committed.
- `targets/raw_triangle.rs`: new `RawTriangleDepth` carries the cells + the
  `Z_CMP`/`Z_UPD`/`ZMODE`/`ZSRCSEL` bits (read off each draw's own snapshotted
  `OtherMode`) + the staged `SetPrimDepth`. In the per-pixel raster loop, the
  fragment's 18-bit working Z (`(prim.z & 0x7fff) << 3` under `G_ZS_PRIM`; the
  triangle's own depth coefficient under `G_ZS_PIXEL`) is compared **strictly
  less-than** against the cell; on a pass the colour writes and (under `Z_UPD`)
  the cell is updated through the RDP exponent/mantissa Z codec.
- `depth_mode.rs` gained `encode_z`/`decode_z`/`encode_delta_z` (public N64
  Programming Manual Ch.16 formulas, mirroring the already-present
  `decode_delta_z`/`relations`/`mode_passes` port) for the quantized update.

### Why strict less-than, and why the compare cases draw nothing

The z-image at guest `0x9000` is the zeroed RDRAM region (never filled by a
depth clear in this corpus), so every cell decodes to working Z 0 -- the
nearest representable. Under a strict less-than compare no `Z_CMP` fragment is
strictly nearer, so a z-compared draw over a freshly-bound z-image draws
nothing -- which is exactly angrylion's output for the five compare cases
(background `0xffff` across the whole 4x3 box). `compare-disabled` keeps
painter's order and matches too. Strict less-than is also fn64's own
documented convention on the GPU pipeline path
(`targets::triangle_pipeline`: "non-inclusive less-than compare op", `Less`).

The four `ZMODE` relations (`mode_passes`) are intentionally NOT dispatched:
the admitted subset carries only `ZMODE_OPAQUE`, and choosing a per-mode
relation this corpus cannot exercise would be an unverified guess. `d.mode`
is retained so a future ZMODE-bearing case widens it by name, not silently.

## Honest residuals / scope notes

- **0-diff on all 6.** No suppression.
- **G_ZS_PIXEL** uses the depth coefficient block's *base* Z integer part
  (`words[0].w0() >> 16`), which is exact for the admitted subset's *flat* Z
  (all deltas zero). A future per-pixel Z-plane interpolation would extend
  `RawTriangleDepth::fragment_z`; this corpus has none, so it is left narrow
  and documented rather than guessed.
- **Z source gate.** The depth accumulator is keyed off the draws' `Z_CMP`/
  `Z_UPD` bits rather than off the z-image address. In the admitted subset
  those bits are only ever set in a packet that also bound a z-image (verified:
  every non-z case's `SetOtherMode` low word is 0), so this is equivalent here
  without threading the address through the neutral IR. Documented at the
  allocation site.

## Verification

- Build: `cargo build -p fn64-render-conformance --features parity-runner ...`
  (green).
- Generate triage: all 6 z cases `wgpu_vs_angrylion_diff_pixels: 0`.
- Gate: `python3 scripts/check_rt64_parity.py` -> `PASS` (exit 0, 33/37
  rt64-authoritative byte-identical; the 4 listed are pre-existing asserted
  exceptions).
- Unit tests (all pass):
  - `raw_dpc::tests::set_mask_image_and_set_z_image_both_decode_to_a_z_image_binding`
    -- 0xfe and 0x3e both decode to one `SetZImage`.
  - `targets::raw_triangle::tests::z_compare_nearer_wins_and_farther_loses_over_a_committed_depth`
    -- pins the compare decision: nearer wins, farther loses over a committed
    depth (reverting the depth test flips the farther-loses assertion).
  - `targets::raw_triangle::tests::z_compare_against_a_zeroed_z_image_rejects_every_fragment`
    -- pins the zeroed-z-image reject behaviour.
- Full `fn64-render-wgpu` suite: 4883 + 13 pass, 0 fail.

## Files changed (all under `crates/fn64-render-wgpu/src/`)

`raw_dpc/mod.rs`, `state.rs`, `raw_dpc/production_adapter.rs`,
`raw_dpc/triangle_draw_data.rs`, `production.rs`, `targets/raw_triangle.rs`,
`targets/mod.rs`, `depth_mode.rs`, `targets/raw_triangle/tests.rs`.
