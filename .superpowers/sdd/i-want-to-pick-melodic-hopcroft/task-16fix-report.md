# Task 16fix report: coverage color-on-cvg write drop (CLR_ON_CVG + CVG_DST_WRAP)

## The exact case
`gen-coverage-color-on-cvg-one-cycle` — the single `fn64-defect` in the
generated coverage corpus. Other-mode low word = `0x080 | 0x100`:
`CLR_ON_CVG` (bit 7) + `CVG_DST_WRAP` (bits 9:8 == 1), with `IM_RD` (bit 6),
`AA_EN` (bit 3), and `FORCE_BL` (bit 14) all **clear**. A one-cycle
flat-shaded triangle pair tiling `[2,6) x [0,3)` = 12 pixels over a
STALE-seeded (`0xffff`) target.

## Before state (confirmed)
- `wgpu_vs_angrylion_diff_pixels: 12`, first diff at x=2,y=0:
  angrylion=`0x2639`, wgpu=`0xffff` (STALE).
- `rt64_vs_angrylion_diff_pixels: 0` (RT64 writes; matches angrylion).
- classification: `fn64-defect` (1 of 1 in the tally).

wgpu dropped **every** covered pixel of the triangle (12/12 stayed STALE);
angrylion + RT64 both wrote all 12.

## Root cause
`blend_and_write_pixel` (`crates/fn64-render-wgpu/src/targets/texrect.rs`,
the executor shared by the raw-triangle path via
`crate::targets::execute_raw_triangle`) gated the color write with:

```rust
if state.other_mode.clear_on_coverage() && !coverage.wraps { return Ok(()); }
```

`coverage.wraps` is the reference's term
`wraps = image_read_enabled && (pixel + memory > 8)`. With `IM_RD` clear it
short-circuits to `false`, so the gate dropped every `CLR_ON_CVG` pixel.

This is wrong versus hardware. In angrylion
(`angrylion-rdp-plus/src/core/n64video/rdp/`), `CLR_ON_CVG`
(`color_on_cvg`) **never gates whether the pixel is written** — the color
write happens iff `blender_1cycle` returns write-enable, which is gated only
by the coverage bit (`blender.c`: `if (antialias_en ? curpixel_cvg :
curpixel_cvbit) { ... return 1 }`). `color_on_cvg` only selects the color
*source* (`!color_on_cvg || prewrap` picks blended vs. straight `blend2a`).
The overflow term angrylion computes is
`prewrap = (curpixel_memcvg + curpixel_cvg) & 8` (`zbuffer.c` `z_compare`) —
computed **without** the `IM_RD` gate; with `IM_RD` clear the no-read
`fbread` returns `memcvg = 0`, leaving `prewrap = curpixel_cvg & 8`. A
full-coverage fragment (`cvg = 8`) always carries out, so it is always
written under `CLR_ON_CVG`.

RT64 doesn't emulate the write-drop at all
(`src/shared/rt64_other_mode.h` reads the bit only for debug display), which
is why RT64 already matched angrylion here.

## The fix
Surgical, one gate in `blend_and_write_pixel`. Replace the `!coverage.wraps`
term with the angrylion carry-out, preserving the `IM_RD`-set path verbatim
so the shared-ported-bug rows do not move:

```rust
let coverage_carry = if state.other_mode.image_read_enabled() {
    coverage.wraps                      // IM_RD path unchanged
} else {
    pixel_coverage.count() & 8 != 0     // full-coverage fragment carries out
};
if state.other_mode.clear_on_coverage() && !coverage_carry { return Ok(()); }
```

Rationale for keeping `coverage.wraps` when `IM_RD` is set: for the
full-pixel fragments this executor produces, angrylion's
`(memcvg + cvg) & 8` carries out exactly when `image_read_enabled && sum > 8`
does, so the `IM_RD`-set behavior (including the `all-modes-combined` /
`force-blend` shared-ported-bug rows) is byte-for-byte unchanged. Only the
`IM_RD`-clear case — the one the defect proves — changes.

`coverage_result`, `CoverageResult`, and the depth path's `coverage.wraps`
consumer were left untouched (no coverage-subsystem restructuring).

## After state (verified)
Targeted triage (`FN64_GENERATE=1 FN64_ONLY=coverage`):
- `gen-coverage-color-on-cvg-one-cycle`: `fn64-defect` ->
  **`pass-all-match-hardware`, `wgpu_vs_angrylion_diff_pixels: 0`**.
- `gen-coverage-all-modes-combined-one-cycle`: `shared-ported-bug`
  (unchanged, 12/12, wgpu=`0x21d7`) — as required.
- `gen-coverage-force-blend-one-cycle`: `shared-ported-bug` (unchanged) — as
  required.
- All 10 other coverage cases: `pass-all-match-hardware` (unchanged).
- triage_counts: `{pass-all-match-hardware: 11, shared-ported-bug: 2}` — no
  `fn64-defect` remaining.

Full gate:
`target/debug/fn64-render-conformance-parity-runner | python3
scripts/check_rt64_parity.py` ->
**`RT64 PARITY GATE: PASS -- 33/37 rt64-authoritative cases byte-identical`**
(the 4 non-matching cases are the pre-existing documented asserted-difference
rows: scissor-narrower-than-rect, textured-rect-yuv16,
perspective-textured-triangle-negative-w, two-cycle-textured).

Full wgpu lib suite: `4885 passed; 0 failed; 3 ignored`.

## Unit test (mutation-guarded)
`clr_on_cvg_with_wrap_writes_a_full_coverage_fragment_without_image_read`
in `crates/fn64-render-wgpu/src/targets/texrect.rs`. Seeds STALE (`0xffff`),
drives `blend_and_write_pixel` with `CLR_ON_CVG | CVG_DST_WRAP` and
`IM_RD`/`AA_EN`/`FORCE_BL` clear + full coverage, asserts the pixel is no
longer STALE. Mutation-verified: reverting the gate to `!coverage.wraps`
fails the test (confirmed by temporary revert).

## Scope / constraints honored
- Only `crates/fn64-render-wgpu/src/targets/texrect.rs` changed (plus this
  report). Pre-existing dirty files (README, Cargo.*, check_rt64_parity.py)
  not committed.
- angrylion referenced as source for the *rule* only; not linked. RT64 read
  for the write-decision confirmation.
- No coverage subsystem restructuring; the fix is one write-decision gate.
