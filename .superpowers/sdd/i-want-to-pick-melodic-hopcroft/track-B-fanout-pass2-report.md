# Track-B fan-out Pass 2 -- regression fix + commit report (task 21)

## Result
- **Gate: PASS -- 33/37** rt64-authoritative cases byte-identical to the RT64 C++ oracle.
- The three regressed `low-t` cases are back to `wgpu_matches_key=True`, `differing_pixels=0`.
- Final generate-triage tally (101 cases): **0 fn64-defect, 0 all-three-differ**.
  - pass-all-match-hardware: 70
  - shared-ported-bug: 22
  - wgpu-refused: 8
  - rt64-hle-defect: 1

## Root cause of the low-t key regression (the exact colliding symbol)
It was an **RDRAM address collision**, not a shadowed Rust symbol.

The three hand-derived skew cases source their texel bars from `SKEW_SOURCE = 0x5000`.
Their seed loop stages source rows `source_y in 94..=108` (low_t 94/95 + 14 rows),
`SKEW_WIDTH = 64` texels each. The highest byte written is
`0x5000 + (108*64 + 63)*2 + 2 = 0x8680`, so the skew source spans **0x5000..0x8680**.

Pass 2 added six new source arrays inside that span and seeded them *after* the
skew rows, clobbering skew source rows >= 0x8000:

| Pass 2 array (old addr) | overlaps skew rows |
|---|---|
| `PALETTE_BANK_CI4_SOURCE` 0x8000 | yes |
| `PALETTE_BANK_TLUT_SOURCE` 0x8100 | yes |
| `CI8_FULL_RANGE_SOURCE` 0x8200 | yes |
| `CI8_FULL_RANGE_TLUT_SOURCE` 0x8300 | yes |
| `TLUT_TYPE_CI8_SOURCE` 0x8400 | yes |
| `TLUT_TYPE_TLUT_SOURCE` 0x8500 | yes |
| `PALETTE_ORIGIN_SOURCE` 0x8600 | yes |

Because the skew cases read source_y from low_t 94/95 upward, the corrupted upper
rows changed the rendered pixels for **both** wgpu and RT64 identically -> the case
stayed `verdict=identical` (wgpu==RT64) but no longer matched its arithmetic
hand key (`skew_expected`) -> `wgpu_matches_key=False`. This exactly reproduces
the reported symptom (deterministic, survives `FN64_ONLY`, render-wgpu untouched):
the pixels changed because shared *guest memory* changed, not because the key or
the renderer changed.

(The old Pass 2 layout was additionally self-inconsistent: `CI8_FULL_RANGE_TLUT`
is 256 entries = 0x200 bytes and would have overrun 0x8300..0x8500 into
`TLUT_TYPE`, and the sparse `TLUT_TYPE_TLUT` writes reached index 0xe0 ->
0x86C0 into `PALETTE_ORIGIN`. Relocation fixes those too.)

## The fix
Relocated the six colliding Pass 2 source constants to a fresh, non-overlapping
region above the skew source and above `MIP1_SOURCE` (0xb000), with a generous
0x400 stride so the 512-byte CI8 TLUT and the sparse TLUT_TYPE array fit without
overlapping each other:

```
PALETTE_BANK_CI4_SOURCE      0xc000
PALETTE_BANK_TLUT_SOURCE     0xc400
CI8_FULL_RANGE_SOURCE        0xc800
CI8_FULL_RANGE_TLUT_SOURCE   0xcc00
TLUT_TYPE_CI8_SOURCE         0xd000
TLUT_TYPE_TLUT_SOURCE        0xd400
PALETTE_ORIGIN_SOURCE        0xd800
PALETTE_ORIGIN_CI8_SOURCE    0xdc00
```

No old case's expected values were touched. The skew key is restored purely by
removing the memory collision -- the correct fix per the brief.

## The 2 all-three-differ construction suspects -> DROPPED
Both stacked a **two-cycle** mode onto a **textured/CI8 triangle** without a valid
two-cycle combine word, so cycle 1 fell into the under-implemented second-cycle
texel path that wgpu and RT64 model differently (the same axis the wgpu-refused
`gen-two-cycle-texel1-*` cases already isolate). Neither tested a clean single
cause, and the two-cycle axis is already covered.

- `gen-blend-deep-two-cycle-textured-bilerp`: wgpu 0x07ff, RT64 0xf801, angrylion
  0x66f7 (wgpu_d=9, rt64_d=12, wgpu != RT64). Dropped; builder `gen_blend_deep_textured`
  removed with it. Two-cycle blender covered by `gen-blend-deep-two-cycle-both-stages`
  (shared-ported-bug) and `gen-two-cycle-wm2000-*`.
- `gen-widerformats-ci8-triangle-two-cycle`: wgpu 0x0001, RT64 0xf801, angrylion
  0x0843 (wgpu_d=12, rt64_d=24, wgpu != RT64) -- directly refuting its own EXPECTED
  "wgpu==RT64" note. Dropped; builder `ci8_textured_triangle_two_cycle` removed with
  it. The #20 CI8+TLUT divergence it claimed is covered cleanly one-cycle by
  `gen-triangle-ci8-bilerp` (shared-ported-bug, wgpu==RT64, d=24).

Both drops carry a code comment at the removed builder and at the push site.

## The 2 huge-diff cases -> one legitimate, one shrunk
- `gen-two-cycle-combined-alpha-chain` (d=320): **legitimate shared-ported-bug**,
  kept. wgpu==RT64 exactly (0x4431 vs angrylion 0x442f = **+1 LSB blue only**),
  over the 40x8+40x8 band region. This is a genuine 1-LSB blend-rounding divergence,
  not whole-framebuffer; d=320 is the band area, not a construction fault.
- `gen-blend-deep-im-rd-striped-framebuffer` (was d=38400 = half the framebuffer):
  the divergence is genuine (wgpu==RT64 = 0x801f vs angrylion 0x781f = **+1 LSB red**,
  the IM_RD blend-rounding path), but the whole-framebuffer triangle pair inflated
  the pixel count with no added signal. **Shrunk** the draw to a 32x8 rect (x 0..32
  still crosses the 16px stripe boundary so both stripe phases are read-modify-written).
  Now **d=128**, same 1-LSB shared divergence, clean signal.

## The known shared-ported-bug domains (all 22 accounted for; no new bug)
- **CI / TLUT S-plane divergence (#20)** -- 12: `gen-tlut-ci4-palette-bank-0/1`,
  `gen-tlut-ci8-full-range-ramp`, `gen-tlut-loadtlut-nonzero-origin`,
  `gen-tlut-type-ia16`, `gen-tlut-type-rgba16`, `gen-triangle-ci4-bilerp`,
  `gen-triangle-ci8-bilerp`, `gen-loadblock-deep-ci8-dxt-fractional-triangle`,
  `gen-loadblock-deep-ci8-dxt400-triangle`, `gen-loadblock-deep-ci8-dxt800-triangle`.
- **FORCE_BL / fog / blender 1-LSB rounding (task-19 log-only)** -- 8:
  `gen-blender-fog-color-over-mem`, `gen-coverage-force-blend-one-cycle`,
  `gen-coverage-all-modes-combined-one-cycle`, `gen-two-cycle-wm2000-shade-fog-program`,
  `gen-two-cycle-combined-alpha-chain`, `gen-blend-deep-blend-color-as-m-mux`,
  `gen-blend-deep-im-rd-striped-framebuffer`, `gen-blend-deep-two-cycle-both-stages`.
- **BI_LERP_0 color-convert (bilerp0 domain)** -- 1 pair: `gen-loadblock-linear-missing-bilerp`,
  `gen-triangle-rgba32-missing-bilerp`.
- **LOD / mip** -- 1: `gen-lod-two-tile-mip-chain-disabled`.

The single `rt64-hle-defect` (`gen-blend-aa-sloped-edge`) and the 8 `wgpu-refused`
cases (two-cycle-texel1 / lod-fraction / alpha-compare-dither / aa-coverage-edge)
are pre-existing Pass 2 capability-gap findings, not regressions.

## Verification commands run
- Build: clean (parity-runner bin: only 3 pre-existing dead-code warnings, none new).
- Gate: `check_rt64_parity.py < gate.json` -> `RT64 PARITY GATE: PASS -- 33/37`.
- Generate triage: 101 cases, 0 fn64-defect, 0 all-three-differ.

## scripts/check_rt64_parity.py
Left the Pass 2 change in place (removal of the obsolete
`textured-rect-flip-point-sampled` entry -- flip is implemented now); committed
together with the runner so the gate is self-consistent.
