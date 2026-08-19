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
