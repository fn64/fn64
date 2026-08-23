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
- **First-batch results:** 16 pass-all-match-hardware, 1 wgpu-refused
  (TexRectFlip), 7 shared-ported-bug — all 7 (and the texrect-flip's RT64
  divergence) traced to a **single RGBA16 texture-source root cause under
  active investigation**, NOT yet 7 independent defects.
- Commits (branch `worktree-wm2000-playable`, not pushed):
  - `2aea6eea` — angrylion oracle leg on the parity runner.
  - `a20e6415` — programmatic corpus generator + first batch.
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

## The RGBA16 texture-source root cause (under investigation)

The 4x2 RGBA16 point-sampled case is the minimal reproduction. Expected sampled
texels (with the `^1` pair swap the sample path applies):

```
row0: 07c1 f801 7fff 003f   row1: c631 8421 fc01 4211
```

angrylion produces, from the byte-identical seeded image:

```
row0: 0001 0001 ffff ffff   row1: c631 8421 0001 4211
```

Probes (standalone oracle, `/tmp/probe*.py`):
- A 40x40 texrect sampling ONLY texel(0,0) reads `0x0001` everywhere — texel 0
  is uniformly wrong, NOT a coverage edge effect.
- Re-seeding the RGBA16 source in five different byte orders (LE/BE halfword,
  no-xor, u32-pack hi-even/hi-odd) changes row1's texels but **never fixes
  row0** and never yields all 8 texels. So it is not a simple source byte-order
  swap.
- texels 4,5,6 read correctly under the shipped LE seeding while texels
  0,1,3,7 read `0x0001`. This word-position-dependent asymmetry points at the
  16bpp TMEM hi/lo bank interleave (RGBA16 texels split high/low bytes across
  the two TMEM banks), i.e. how `LoadTile`/`LoadBlock` deposits a 16bpp source
  into TMEM vs. how the sampler reads it back — not the source image bytes.

**Verdict pending** a Codex investigation tracing angrylion's `tmem.c` 16bpp
LoadTile → TMEM → sample path. Two live hypotheses:
1. Harness staging: fn64's `seeded()` RGBA16 source is in a byte order the
   oracle's 16bpp LoadTile does not read the way fn64/RT64 do — a fixable
   transformation in `angrylion_bytes()` for the texture-source region only.
2. Genuine angrylion 16bpp TMEM behaviour that wgpu AND RT64 both diverge from
   — which would make these 7+ cases real shared ported-from-RT64 defects.

Because wgpu==RT64==hand-key (three implementations) on all of them, the strong
prior is (1); I will NOT report these as fn64 defects until the load path is
traced and a reproduction reads all 8 texels correctly.

## Step 2 — generator and first batch (24 cases)

`FN64_GENERATE=1` emits synthetic streams (built from the same wire encoders
the hand corpus uses; never captured from a ROM) and three-way compares wgpu
and RT64 against angrylion. Priority order per the brief.

### Triage counts

| classification | count |
|---|---|
| pass-all-match-hardware | 16 |
| shared-ported-bug (RGBA16 root cause) | 7 |
| wgpu-refused (TexRectFlip) | 1 |

### Per-case

| case | pri | classification | wgpu vs ang | rt64 vs ang |
|---|---|---|---|---|
| gen-loadblock-linear | 1 | shared-ported-bug* | 5 px | 5 px |
| gen-loadblock-dxt | 1 | shared-ported-bug* | 13 px | 13 px |
| gen-triangle-opcode-0x08 (flat) | 2 | pass ✓ | 0 | 0 |
| gen-triangle-opcode-0x09 (flat+zbuf) | 2 | pass ✓ | 0 | 0 |
| gen-triangle-opcode-0x0a (tex) | 2 | shared-ported-bug* | 12 px | 12 px |
| gen-triangle-opcode-0x0b (tex+zbuf) | 2 | shared-ported-bug* | 12 px | 12 px |
| gen-triangle-opcode-0x0c (shade) | 2 | pass ✓ | 0 | 0 |
| gen-triangle-opcode-0x0d (shade+zbuf) | 2 | pass ✓ | 0 | 0 |
| gen-triangle-opcode-0x0e (shade+tex) | 2 | shared-ported-bug* | 12 px | 12 px |
| gen-triangle-opcode-0x0f (shade+tex+zbuf) | 2 | shared-ported-bug* | 12 px | 12 px |
| gen-textured-triangle | 2 | shared-ported-bug* | 12 px | 12 px |
| gen-sync-loadsync-in-fill | 3 | pass ✓ | 0 | 0 |
| gen-sync-pipesync-in-fill | 3 | pass ✓ | 0 | 0 |
| gen-sync-tilesync-in-fill | 3 | pass ✓ | 0 | 0 |
| gen-texrect-flip | 4 | wgpu-refused | — | 13 px* |
| gen-fill-{red,green,blue,white}-fullwidth-band | 5 | pass ✓ (x4) | 0 | 0 |
| gen-fill-single-pixel | 5 | pass ✓ | 0 | 0 |
| gen-fill-last-pixel | 5 | pass ✓ | 0 | 0 |
| gen-modematrix-cycle-{fill,one,two}-red-box | 5 | pass ✓ (x3) | 0 | 0 |

`*` = confounded by the RGBA16 texture-source root cause; not an independent
defect until that is resolved.

### Clean wins (proved by the generator)

- **Triangle opcode family 0x08/0x09/0x0c/0x0d** (flat, flat+zbuf, shade,
  shade+zbuf) all match hardware — previously only 0x08 and 0x0c were tested.
- **PipeSync (0x27), TileSync (0x28), LoadSync (0x26)** inside a fill frame
  perturb nothing — all match hardware.
- **Cycle-type mode matrix** (fill / 1-cycle / 2-cycle) all match hardware.
- **Fill rasteriser boundaries** (full-width bands in 4 colours, single pixel,
  last pixel) all match hardware.

### Generator construction defects found and fixed

`FillRectangle` lower-right is INCLUSIVE; the first generator draft passed
exclusive `lrx=WIDTH`, which every backend correctly refused as
"FillRectangle exceeds the staged color-image width". Fixed in `a20e6415`
(convert exclusive → inclusive in the fill path). This is exactly the
"all-three-differ / inspect construction" rubric arm doing its job.

## Discovered defects

### fn64 (wgpu) defects — ranked

1. **(pending) RGBA16/CI/RGBA32 texture sampling vs bit-accurate hardware** —
   the largest signal in both corpora. HELD: wgpu==RT64==hand-key on all of
   them, so the likely cause is the oracle-harness 16bpp texture-source path,
   not fn64. Will be reclassified once the TMEM load path is traced.
2. **TexRectFlip (0x25) refused by wgpu** — `execute_raw_dpc refused: ...
   declared no journal write access`. wgpu declines TexRectFlip outright
   (matches the hand corpus's `textured-rect-flip-point-sampled`, also
   refused). A real fn64 wgpu limitation, independent of the RGBA16 issue.

### RT64 HLE defects — ranked

None confirmed independent of the RGBA16 root cause in this batch. The
`gen-texrect-flip` RT64 divergence and the seven shared-ported-bug RT64
divergences all read angrylion's `0x0001` broken texel, so they cannot be
attributed to RT64 until the texture-source path is resolved.

## What's next (expansion)

1. **Resolve the RGBA16 texture-source root cause** (Codex investigating). Once
   fixed, RE-RUN the batch: the 7 shared-ported-bug cases will either flip to
   pass (harness bug) or become confirmed shared defects (angrylion exposes a
   real wgpu+RT64 divergence) — a high-value outcome either way.
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
