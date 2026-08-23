# Track B: programmatic RDP parity corpus generator — report

Owner: Track B (angrylion-grounded parity corpus).
Worktree: `/Users/jer/Code/fn64/.claude/worktrees/wm2000-playable`, branch
`worktree-wm2000-playable`.

## Summary

- **Angrylion leg added and validated** against the 39 hand cases. The oracle's
  raw framebuffer bytes are byte-identical to fn64's `observation_bytes` domain
  (verified: `BYTE_ADDR_XOR=3`/`WORD_ADDR_XOR=1`/word-un-XORed == fn64's
  `^3`/`^2`/none storage swizzle). All fills, scissor, IA/I textured, and
  flat/shade triangles agree with angrylion.
- **Generator built** (`FN64_GENERATE=1` mode on the parity runner). First
  batch: **24 synthetic cases** across the priority matrix, three-way compared
  against angrylion ground truth with a triage rubric.
- **First-batch results (after root-cause fix): 23 pass-all-match-hardware,
  1 confirmed fn64 defect (wgpu refuses TexRectFlip), 1 intentional
  bilerp-gap witness.** The 7 initial "shared-ported-bug" RGBA16 cases were
  ROOT-CAUSED and RESOLVED to a missing `BI_LERP_0` mode bit in the corpus's
  textured SetOtherModes (a fixture gap, not an fn64 defect) — proven by
  instrumenting angrylion; setting the bit makes all three backends agree.
- Commits (branch `worktree-wm2000-playable`, not pushed):
  - `2aea6eea` — angrylion oracle leg on the parity runner.
  - `a20e6415` — programmatic corpus generator + first batch.
  - `1ccc15c5` — RGBA16 root cause (BI_LERP_0) + generator fix + witness.
  - External oracle (`/Users/jer/Code/angrylion-oracle`, not under git) gained a
    `--rdram` full-image mode; documented in its `BUILD-NOTES.md`.

## Step 1 — angrylion leg validation matrix (39 hand cases)

The leg hands the oracle the full `seeded()` RDRAM image (STALE background,
GUARDs, staged texture sources, and the command words) via the oracle's new
`--rdram` mode, so angrylion renders from byte-identical guest memory to wgpu
and RT64. The earlier cmds-only oracle mode seeded a zeroed RDRAM and read
texel 0 everywhere for any textured draw; that is why the full image is
required.

Agreement with angrylion (`wgpu==angrylion`), of 39:

| bucket | cases | agree with angrylion |
|---|---|---|
| Fills / scissor / nested / order | full-target-red, right-half-blue, top-left-quadrant, single-pixel, last-column-last-row, even-color-lsb-clear, nested-second-fill, scissor-top-rows-only, three-fills-strict-order, one-cycle-fill-band, coverage-aa-enabled-fill | ALL agree ✓ |
| IA / I textured | ia8, ia4, ia16, i4, i8 | ALL agree ✓ |
| Flat / shade triangle | flat-triangle-primitive, shade-only-triangle | agree ✓ |
| **RGBA16 / CI / RGBA32 textured** | point-sampled, second-row-only, ci4-tlut, ci8-tlut, rgba32, loadblock-linear, loadblock-dxt, textured-triangle-point-sampled, wide-line-two, line16/17-low-t | **wgpu==RT64==key, angrylion differs** (see root cause) |
| blend / fog | blend-numerator-overflow-wrap, blend-color-blender-passthrough, fog-color-blender | angrylion differs (same texture-source path — these draw a texrect) |
| Refused in wgpu (state-invalid by design) | scissor-narrower-than-rect, two-cycle-textured, perspective-textured-triangle-negative-w, coverage-alpha-dither-enabled, textured-rect-yuv16, textured-rect-flip-point-sampled | one/both lanes refuse — pre-existing |

18 of 36 renderable hand cases have `wgpu==angrylion`. Critically, **every case
where angrylion disagrees is an RGBA16/CI/RGBA32 texture-source case**, and in
every one wgpu==RT64==hand-key (three independent implementations agree). This
is the signature of a single shared cause, tracked below — the leg itself is
validated (fills match the key; IA/I/flat/shade/scissor textured cases match
wgpu+RT64).

**RT64-vs-angrylion divergences found in the hand corpus:** none independent of
the RGBA16 root cause. (`two-cycle-textured` refuses in wgpu and differs in
RT64, but it too draws an RGBA16 texrect.)

## RESOLVED — RGBA16 texture root cause: missing BI_LERP_0 mode bit

**Verdict: a fixture/mode-completeness bug in the parity corpus's textured
`SetOtherModes`, NOT an fn64 rendering defect and NOT a source-staging bug.**
Proven by instrumenting a throwaway angrylion build (the canonical angrylion
tree and the production oracle stay pristine; all instrumentation lives in
`/Users/jer/Code/angrylion-oracle/scratch/`).

Localization, step by step, for the 4x2 RGBA16 point-sampled case:
- **LOAD is correct.** Dumping TMEM after `rdp_load_tile` shows all 8 texels
  present: low-bank words `07c1 f801 7fff 003f fc01 4211 c631 8421` (the
  word-swapped TMEM layout angrylion uses).
- **FETCH is correct.** Instrumenting the RGBA16 texel fetch shows every pixel
  reads its right texel: `s=0,t=0 -> f801`, `s=1 -> 07c1`, ... all 8 correct.
- **The collapse is in the texel pipeline, BEFORE the combiner.** The texel
  reaching the combiner is grayscale `(v,v,v,v)` where `v` is the texel's BLUE
  channel. `tex.c` routes a NON-bilerp texel through the color-convert/YUV
  formula `TEX->r = t0.b + (k0*t0.g>>8)`, etc.; with SetConvert coefficients
  zero this is `TEX->r = TEX->g = TEX->b = TEX->a = t0.b`.
- **The gate is `other_modes.bi_lerp0 = (SetOtherModes word0 >> 11) & 1`.** The
  corpus's `OTHER_MODES_ONE_CYCLE_TEXTURED = 0xef0000f0` leaves bit 11 clear,
  so angrylion takes the convert path and collapses to blue.

**Confirmed fix:** setting bit 11 (`0xef0000f0 -> 0xef0008f0`) in the stream
makes angrylion output byte-identical to the hand-derived key:
`row0 = [07c1, f801, 7fff, 003f]`, `row1 = [c631, 8421, fc01, 4211]`.

**Why the pass/fail split:** IA/I textures already carry intensity in the blue
channel, so the collapse is a no-op — that is exactly why IA8/IA16/I4/I8 passed
while every RGBA16/CI/RGBA32 case failed. fn64's wgpu and RT64 both IGNORE the
missing BI_LERP bit and pass the full RGBA texel through, which is why all three
(wgpu, RT64, hand-key) agree with each other yet diverge from bit-accurate
hardware.

**Consequence for the corpus:** the corpus's textured `SetOtherModes` is
missing BI_LERP_0. This is a genuine finding the angrylion leg surfaced — the
hand cases' expected keys match wgpu+RT64 but would NOT match real hardware on
this bit. Per the brief I do not modify the existing hand cases; the fix belongs
in the shared textured other-modes (and is applied in the generator's textured
cases going forward). Whether wgpu/RT64 SHOULD honor BI_LERP_0 (and currently
mask a real behavior) is a separate question worth raising with the RT64/wgpu
owners.

## Step 2 — generator and first batch (25 cases)

`FN64_GENERATE=1` emits synthetic streams (built from the same wire encoders
the hand corpus uses; never captured from a ROM) and three-way compares wgpu
and RT64 against angrylion. Priority order per the brief. The textured cases
carry the BI_LERP_0 correction from the root cause above; one uncorrected
witness is kept to keep the finding reproducible.

### Triage counts (final, after the BI_LERP_0 fix)

| classification | count |
|---|---|
| pass-all-match-hardware | 23 |
| wgpu-refused (TexRectFlip — confirmed fn64 defect) | 1 |
| shared-ported-bug (intentional bilerp-gap witness) | 1 |

Before the fix the counts were 16 pass / 7 shared-ported-bug / 1 wgpu-refused;
the 7 were all RGBA16-texture cases and all resolved to the single BI_LERP_0
root cause.

### Non-passing cases (final)

| case | pri | classification | note |
|---|---|---|---|
| gen-texrect-flip | 4 | wgpu-refused | wgpu declines TexRectFlip (`execute_raw_dpc refused ... no journal write access`); RT64 and angrylion both render it and AGREE (0 px diff). A real fn64 wgpu gap. |
| gen-loadblock-linear-missing-bilerp | 1 | shared-ported-bug | INTENTIONAL witness: the same LoadBlock WITHOUT BI_LERP_0. wgpu==RT64 (5 px) vs angrylion — reproduces the bilerp gap as a live row. |

All other 23 cases (triangle opcode family 0x08-0x0f including the textured and
zbuffered variants, PipeSync/TileSync/LoadSync, cycle-type mode matrix, fill
rasteriser boundaries, LoadBlock linear + DxT, textured triangle) are
pass-all-match-hardware.

### Clean wins (proved by the generator)

- **Triangle opcode family 0x08/0x09/0x0c/0x0d** (flat, flat+zbuf, shade,
  shade+zbuf) all match hardware — previously only 0x08 and 0x0c were tested.
- **PipeSync (0x27), TileSync (0x28), LoadSync (0x26)** inside a fill frame
  perturb nothing — all match hardware.
- **Cycle-type mode matrix** (fill / 1-cycle / 2-cycle) all match hardware.
- **Fill rasteriser boundaries** (full-width bands in 4 colours, single pixel,
  last pixel) all match hardware.
- **LoadBlock (linear + DxT) and textured triangles** match hardware once
  BI_LERP_0 is set — confirming both the fetch path and the root-cause fix.

### Generator construction defects found and fixed

`FillRectangle` lower-right is INCLUSIVE; the first generator draft passed
exclusive `lrx=WIDTH`, which every backend correctly refused as
"FillRectangle exceeds the staged color-image width". Fixed in `a20e6415`
(convert exclusive → inclusive in the fill path). This is exactly the
"all-three-differ / inspect construction" rubric arm doing its job.

## Discovered defects

### fn64 (wgpu) defects — ranked

1. **TexRectFlip (0x25) refused by wgpu — CONFIRMED.**
   `execute_raw_dpc refused: ... texture-rectangle triangle declared no journal
   write access`. wgpu declines TexRectFlip outright, while RT64 AND angrylion
   both render it and agree pixel-for-pixel (0 px diff, with BI_LERP_0 set).
   This is the one clean, isolated fn64 rendering gap in the batch. It also
   matches the hand corpus's refused `textured-rect-flip-point-sampled`.

### Corpus / fixture findings (surfaced by the angrylion leg) — ranked

1. **Textured `SetOtherModes` omits `BI_LERP_0` (bit 11) — corpus-wide.**
   Every textured hand case and the textured generator builders use
   `OTHER_MODES_ONE_CYCLE_TEXTURED = 0xef0000f0`, which leaves bit 11 clear.
   Bit-accurate hardware then routes RGBA/CI/RGBA32 texels through the
   colour-convert unit and collapses them to the blue channel; wgpu and RT64
   both ignore the bit and pass full colour, so they match each other and the
   hand key but diverge from hardware. Proven and fixed in the generator
   (`set_bilerp0`). The hand cases are left unmodified per the brief, but their
   keys are hardware-incorrect on this bit. **Open question worth raising with
   the RT64/wgpu owners:** should wgpu and RT64 honour BI_LERP_0 (they currently
   mask a real hardware behaviour)? If yes, this becomes a shared wgpu+RT64
   defect; if the intended real-ROM streams always set the bit, it is purely a
   fixture gap.

### RT64 HLE defects — ranked

None confirmed in this batch. With BI_LERP_0 set, RT64 agrees with angrylion on
every renderable case, including TexRectFlip (which wgpu refuses).

## What's next (expansion)

1. **DONE — RGBA16 root cause resolved (BI_LERP_0).** The 7 cases flipped to
   pass with the mode bit set. Follow-up: decide with the RT64/wgpu owners
   whether wgpu+RT64 should honour BI_LERP_0 (they currently mask the hardware
   collapse); if they should, add a corpus row asserting the collapse and file
   it as a shared wgpu+RT64 defect.
2. **Investigate the TexRectFlip wgpu refusal** — RT64 and angrylion render it
   and agree, so wgpu is the outlier. Confirmed fn64 gap; scope a fix.
2. Expand priority (1): LOADBLOCK DxT stride variants, larger blocks, multi-row.
3. Expand priority (4): SetPrimDepth (0x2e) Z values, SetBlendColor (0x39)
   blend-color paths, TexRectFlip once wgpu support is assessed.
4. Expand priority (5): full mode matrix — blend mode × alpha compare ×
   coverage mode × dither, crossed with FillRect/TexRect/Triangle.
5. Expand priority (6): SetConvert/SetKey (0x2a/2b/2c), SetMaskImage (0x3e).

## How to reproduce

```
FN64_RT64_DIR=$HOME/Code/no-mercy-recompiled/third_party/rt64 \
  cargo build -p fn64-render-conformance --features parity-runner \
  --bin fn64-render-conformance-parity-runner --offline

# hand corpus + angrylion leg:
FN64_RT64_DIR=... ./target/debug/fn64-render-conformance-parity-runner
# generated corpus:
FN64_GENERATE=1 FN64_RT64_DIR=... ./target/debug/fn64-render-conformance-parity-runner
# dump one case's seeded RDRAM for standalone oracle replay:
FN64_DUMP_CASE=<name> FN64_RT64_DIR=... ./target/debug/fn64-render-conformance-parity-runner
```

Angrylion oracle overridable via `FN64_ANGRYLION_ORACLE` (default
`/Users/jer/Code/angrylion-oracle/oracle`); missing binary → the leg skips with
a logged sentinel, never a build/test failure.
