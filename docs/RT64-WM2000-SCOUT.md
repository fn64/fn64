# WM2000 all-Rust lane: scout report

**Everything past Wall 1 was obtained with guards deliberately disabled.
This is a PREVIEW of the wall order, not a validated path.** No production
fix in this document has been verified; every "disabled" hack is throwaway
and lives only on the `scout/wall-preview` scratch branch.

Base commit: `201dfb92`.

Lane: `FN64=/private/tmp/fn64-scout ~/Code/recomps/wm2000/packages/wm2000-boot/rs/run-rs-lane.sh`
(`FN64_RECOMP=rs`, `FN64_RENDER=wgpu`, no `--features rt64`).

## Summary — the wall order, deepest reach, and what it means

**Deepest reach with every guard disabled**: the run clears the old abort
and reaches WM2000's progress heartbeat repeatedly:

```
[wm2000-boot] progress: steps=50000  sim_time=895940131  vi_swaps=280 gfx_tasks=280  audio_tasks=527
[wm2000-boot] progress: steps=100000 sim_time=1757449399 vi_swaps=555 gfx_tasks=1079 audio_tasks=1031
```

**555 VI swaps and 1,079 gfx tasks at 100,000 steps, still running.** The
first heartbeat appears in every run from Wall 2 onward — the aborts
below all happen *after* 50,000 steps of real execution, not during boot.
The **second** heartbeat appears only in run 6, with all six guards
disabled: that is the run where the texrect path stopped aborting and
started sustaining. Swaps are still accumulating (280 -> 555) and gfx
tasks are accelerating (280 -> 1,079), so the DPC path is being driven
continuously, not stalled.

Walls found, in the order the run hits them:

| # | Refusal | Subsystem | Divergence row | Guess |
|---|---|---|---|---|
| 1 | `TmemSampleFailed{status:2}` | GPU shader | post-D22 | real defect (owned by another lane) |
| 2 | `InvalidTexelByte 0x08a` | CPU texel read | not a divergence (audit `:813`) | correct refusal, TMEM coverage bug behind it, owned elsewhere |
| 3 | `EnabledCiSourceOutsideLowHalf 0x800` | CPU texel read | **D14** | invented constraint; reference right |
| 4 | `NonCanonicalTlutEntry 0x948` | CPU texel read | **D13** | wgpu writes a state it refuses to read |
| 5 | `IncompleteTlutEntry 0xa98 mask 0x0f` | CPU texel read | **D13** (2nd half) | UNKNOWN; wgpu strictly stricter |
| 6 | `NoBlendColor` (triangle #0) | plan admission | **new** | RDP register modelled `Option`; reference zero-inits |
| 7 | `UnsupportedColorInput{ShadeAlpha}` | texrect combiner | **D4** | real defect; `combiner.rs` implements what the executor refuses |

**Deepest reach of all: 400,000 steps / 2,149 VI swaps / 5,967 gfx tasks**
with all seven guards disabled and no eighth wall — over half of the
attract loop's ~3,990 swaps.

**A frame with real content was captured** at swap 220 — see
[A CAPTURED FRAME WITH REAL CONTENT](#a-captured-frame-with-real-content).
The composition and the attract-loop fade are correct; the texel rows are
striped at a one-pixel period, the visible signature of the row-parity
defect.

Read the [cross-reference section](#cross-reference-the-audit-predicted-walls-3-4-and-5)
before acting on any per-wall guess — it revises several of them.

**Everything from Wall 2 down was reached with guards disabled and is a
preview, not a validated path.**

---

## Wall 1 — GPU tmem_sample status 2 (IA4 under G_TT_RGBA16)

- **Error**: `execute_raw_dpc: render-wgpu/raw-dpc-execute backend error: a
  triangle draw's fragment shader reported a non-OK tmem_sample.wgsl status: 2
  (triangle #0 in plan order, tile format code 3, pixel-size code 0, TLUT-mode
  code 2)`
- **Site**: `crates/fn64-render-wgpu/src/production.rs:612`
  (`draw_admitted_triangles`, post-readback status check).
- **Subsystem**: GPU shader (`tmem_sample.wgsl`) status readback.
- **Disabled**: the whole `if let Some(&status) = output.tmem_sample_status…`
  block short-circuited (`if false`).
- **Guess**: real defect — another lane (`/private/tmp/fn64-gpubyte`) is
  fixing the WGSL sampler's hardcoded row parity. Shape: *CPU/GPU
  disagreement on the same texture*, already 2-for-2 today.
- **Known divergence**: to be cross-referenced.

## Structural finding S1 — the GPU raster path has no RDRAM writeback

Found by reading, before any run reached it, and it is not a "wall" the
run will abort on — it is a **silent** gap that makes the GPU triangle
lane invisible no matter how many shader walls are cleared.

- `crates/fn64-render-wgpu/src/production.rs` (`stage_and_report`'s D24
  comment block, ~line 2773) states it in its own words: *"The missing
  RDRAM writeback for the GPU raster path is a separate, pre-existing gap
  that this arm never closed."*
- A `RawTriangle` pushes **no `ResourceAccess`** at all; the decoder's
  `0x08..=0x0f` arm decodes and pushes the command but calls no planner.
  So it declares no journal write and stages no `CompletedWrite`.
- Its raster lands in `triangle_draw_output`, which `present`
  (`production.rs:1949`) refuses to scan out by name: *"one submission's
  readback, not a VI-sampled framebuffer"*.
- **Consequence for this card's goal.** Texrects (the CPU-composed path
  through `stage_color_commands` -> `ColorTargetRegistry` ->
  `copy_committed_guest_writes`) ARE guest-visible. Raw triangles are not.
  WM2000's title/HUD path is texrects (2,520 measured), so a *frame* is
  reachable without closing S1 — but any 3D geometry is not.
- **Guess: real, acknowledged gap** (the crate says so itself), not a
  refusal. It needs a design decision (readback-and-copy vs. rastering
  into the color target), not a one-line widening.

## Predicted-wall inventory (read, not yet reached)

Refusal surfaces that sit downstream of the current abort, listed so the
next cards can be written before the run reaches them. **These are read
from source, not measured**; a refusal listed here may never fire.

### P1 — `TexrectExecutionError`, 21 named variants
`crates/fn64-render-wgpu/src/targets/texrect.rs`. WM2000's title path is
2,520 texrects, so every one of these sits directly in the goal path:

`UnsupportedCycleType`, `UnsupportedColorInput`, `UnsupportedAlphaInput`,
`UnsetConstantRegister`, `NoDeclaredRows`, `NegativeViewportOrigin`,
`EmptyViewport`, `NonIntegralTexcoord`, `TexcoordOutOfRange`,
`OutsideTarget`, `UnboundTile`, `MissingResidentBytes`, `Sample`,
`NoiseThresholdUnavailable`, `OrderedDitherAuthorityUnsettled`,
`DestinationCoverageUnavailable`, `ReservedAlphaCompare`,
`UnsupportedBlendShadeAlpha`, `UnsupportedBlendFramebufferAlpha`,
`BlendEnabledNotDerivable`, `Blend`, `Target`.

Cross-reference: `UnsupportedCycleType{Fill}` is divergence **D3**
(fill-cycle texrect) — the row the audit calls "unmeasured for WM2000;
broke a sibling ROM (WCW/nWo Revenge)". `UnsupportedColorInput` /
`UnsupportedAlphaInput` are **D4**; `BlendEnabledNotDerivable` is **D5**;
`OrderedDitherAuthorityUnsettled` is **D7**.

### P2 — `ViScanoutRefusal`, 7 named variants
`crates/fn64-render-wgpu/src/vi_scanout.rs:71`: `SilhouetteAntialias`
(**D1**), `DitherRestorationNonRgba16`, `Divot`, `Gamma`, `Fade`,
`RepeatLine`, `ReservedPixelType`.

**Correction to D1's "REACHES WM2000: FIRST FRAME" claim.** The module's
own measurement (`vi_scanout.rs:302-316`) says WM2000's first content
field (field 20) latches `status=0x00013202` = `ViAaMode::ResampleOnly`
(AA mode **2**), with divot/gamma/fade/repeat-line all clear. Silhouette
AA (modes 0/1) is therefore **not** selected in the measured window, and
both filters WM2000 does select (dither restoration, resample) are
already implemented. Guess: D1's Tier-A placement is stale relative to
this measurement.

`Fade` deserves a flag: the attract loop is content -> white -> content
over ~3,990 swaps. If that white is `osViFade` rather than a drawn white
frame, `ViScanoutRefusal::Fade` fires mid-attract. Unverified.

---

## Wall 2 — CPU texel reader: physical TMEM byte 0x08a never written · **PAST THE FIRST HEARTBEAT**

**Milestone reached first.** With Wall 1 neutered the run got far past the
old abort:

```
[wm2000-boot] progress: steps=50000 sim_time=895940131 vi_swaps=280
              gfx_tasks=280 audio_tasks=527
```

**280 VI swaps and 280 gfx tasks before the next abort.** This is the
first time the all-Rust lane has printed a heartbeat at all.

- **Error**: `execute_raw_dpc: render-wgpu/raw-dpc-execute backend error:
  texture rectangle texel fetch failed at pixel (0, 0): physical TMEM
  texel byte 0x08a is invalid`
- **Site**: raised at `crates/fn64-render-wgpu/src/tmem/read.rs:573`
  (`read_valid_byte` -> `PhysicalTexelReadError::InvalidTexelByte`),
  reached through `read_linear_bytes`; surfaces at
  `crates/fn64-abi/src/task_dispatch/rsp_commit.rs:1202`.
- **Subsystem**: **CPU texel read** (not the GPU shader).
- **Semantics**: `TmemByteSource::valid_byte(0x08a)` returned `None` —
  the byte was never written by any load in this TMEM state. The reader
  refuses to invent a value.

**This is the important classification.** Wall 1's GPU status 2 is
`TMEM_SAMPLE_STATUS_INVALID_BYTE`, and `tmem_sample.wgsl:149` says so:
*"Matches `PhysicalTexelReadError::InvalidTexelByte`."* Wall 2 is the CPU
reader reporting **the same condition on the same class of texture**.

**Therefore Wall 1 is probably NOT a CPU/GPU disagreement.** Both halves
agree the addressed byte is unwritten. The shape here is not the
two-for-two palette/row-parity shape; it is a **third** party — either
the addressing arithmetic (`linear_byte_address` + `odd_row_exchange`) is
computing an address outside what the load wrote, or the load itself
under-populated TMEM. **Guess, marked as a guess**: the row-parity fix in
flight on `/private/tmp/fn64-gpubyte` may move the GPU to agree with the
CPU *and still be invalid*, i.e. clearing Wall 1 may land directly on
Wall 2 rather than on a frame.

- **What was disabled to get past Wall 1**: the shader-status check block
  in `production.rs:600-620` (`find(|&&status| false && ...)`).
- **Known divergence**: **new**. No row of `RT64-LANE-DIVERGENCES.md`
  covers an unwritten-TMEM-byte fetch; the audit's TMEM rows are about
  format/palette handling, not byte validity coverage.

---

## Wall 3 — CI-under-TLUT source byte 0x800 refused instead of wrapped

- **Error**: `execute_raw_dpc: render-wgpu/raw-dpc-execute backend error:
  texture rectangle texel fetch failed at pixel (3, 14): enabled-TLUT CI
  source byte 0x800 is outside canonical low-half TMEM`
- **Site**: `crates/fn64-render-wgpu/src/tmem/read.rs:496`
  (`validate_address_scope` -> `EnabledCiSourceOutsideLowHalf`).
- **Subsystem**: CPU texel read, address-scope preflight.
- **Disabled**: the `if address >= TMEM_HIGH_HALF_BASE { return Err(...) }`
  arm.
- **Reached at**: pixel (3, 14) — i.e. the reader got through 3 columns and
  14 rows of a rectangle before the address walked past 0x7ff.

**Strong guess: a real defect, and its refutation is two functions away in
the same file.**

`first_physical_byte` (read.rs:609) has two arms with different masks:

```
PixelSize::Bits32 -> rgba32_low_address(...)   // masks TMEM_LOW_HALF_MASK
otherwise         -> linear & TMEM_ADDRESS_MASK  // masks 0x0fff, full 4 KB
```

So RGBA32 — which has the *same* low-half-only constraint — **wraps** to
the low half by masking. The CI-under-TLUT path takes the 0x0fff mask,
then `validate_address_scope` refuses the result for being ≥ 0x800. One
constraint, two treatments, in adjacent functions.

Note the address is **exactly 0x800**, the first byte past the boundary,
at row 14 of a rectangle. That is what an address walking off the end of
the low half looks like — not a wild pointer. Whether hardware wraps
(mask to 0x7ff) or the tile's `line_words`/base is being mis-derived
upstream is the question the next card must answer; I did not measure it.

- **Shape match**: *a refusal whose sibling in the same file does the
  opposite* — a variant of "a refusal whose doc comment contradicts its
  code", and adjacent to the fill-cycle-texrect case where widening
  would have been wrong. **Do not widen this one blind**: if hardware
  does NOT wrap, masking would sample the TLUT as image data.
- **Known divergence**: **new**. Not in `RT64-LANE-DIVERGENCES.md`.
- **Related to Wall 2**: same texrect path, same addressing arithmetic
  (`linear_byte_address` + `odd_row_exchange`). Walls 2 and 3 are
  plausibly **one** underlying addressing defect surfacing twice — an
  address that runs off the loaded region (Wall 2: unwritten byte) and
  off the low half (Wall 3: 0x800). **Guess.**

### Wall 2 addendum — it is NOT the row-parity bug that was already fixed

`crates/fn64-render-wgpu/src/targets/texrect.rs:1225-1241` records that the
CPU side once passed a **frozen `TmemFirstRowParity::Even`**, that WM2000's
sprite-strip tile has `low_t.integer() == 47` (odd), and that the frozen
constant made *"each rectangle row's last pixel read a byte the load never
wrote"* — **abort byte `0x04c`**, pinned by two `wm2000_texrect_*` tests.

That fix is in the tree I am running. Wall 2's byte is **`0x08a`**, a
different address, at pixel **(0, 0)** — the *first* pixel, not a row's last.
So Wall 2 is a **second, distinct** instance of "the reader addresses a byte
no load wrote", not a regression of the parity fix. **Guess**: the tile base
(`tile.tmem()`) or `line_words` for this particular tile disagrees with what
the load populated, rather than the row walk being off.

---

## Wall 4 — TLUT entry at 0x948 is not quadricated: lanes `[0100, 0100, 0100, 8f94]`

- **Error**: `execute_raw_dpc: render-wgpu/raw-dpc-execute backend error:
  texture rectangle texel fetch failed at pixel (0, 0): TLUT entry at 0x948
  is not four equal big-endian 16-bit lanes: [0100, 0100, 0100, 8f94]`
- **Site**: `crates/fn64-render-wgpu/src/tmem/read.rs:597`
  (`read_canonical_tlut_entry` -> `NonCanonicalTlutEntry`).
- **Subsystem**: CPU texel read, TLUT entry decode.
- **Disabled**: the `lanes[1..].iter().any(...)` canonicality check
  (take `lanes[0]`, matching what the RDP's palette read does).
- **What the numbers say.** The RDP quadricates on `LoadTLUT`: one 16-bit
  palette entry is written into all four 16-bit lanes of the 64-bit word.
  Three lanes here hold `0x0100`; the fourth holds `0x8f94`. So the word
  is **three-quarters one entry and one-quarter something else** — a
  partially-overwritten TLUT word, not garbage and not a decode error.
- **Guess: real defect, and the strongest single lead in this report.**
  A quadricated word cannot end up 3/4-consistent by accident. Either
  (a) a `LoadTLUT` populated only 3 of 4 lanes and lane 3 retains an
  older load's byte pair, or (b) a `LoadBlock`/`LoadTile` wrote image
  data into TMEM high half at 0x94e and clobbered lane 3 of a TLUT word.
  Note `0x8f94` is plausible RGBA16 image data, not a palette index.
  **This is the same family as Walls 2 and 3**: the TMEM write side is
  populating a region differently from how the read side addresses it.
- **Known divergence**: **new**. Not in `RT64-LANE-DIVERGENCES.md`.
- **Shape**: not one of the six listed. Closest is *CPU/GPU disagreement*,
  but this is CPU-vs-CPU: the loader and the reader disagree about the
  same 8 bytes.

### Wall 4 addendum — the loader cannot produce this word, so a second writer did

`crates/fn64-render-wgpu/src/tmem/execute/load_tlut.rs:403-420`
(`map_physical_lanes`) writes `[hi, lo]` into **all four** lanes
unconditionally:

```
Ok([Some(*hi), Some(*lo), Some(*hi), Some(*lo),
    Some(*hi), Some(*lo), Some(*hi), Some(*lo)])
```

There is no partial arm and no early return. **A `LoadTLUT` alone can
never leave a 3/4-consistent word.** Hypothesis (a) from the entry above
is therefore refuted by reading; hypothesis (b) stands:

> something other than the TLUT loader wrote bytes 0x94e-0x94f.

0x948 sits in TMEM's **high half** (0x800-0xFFF), which is where TLUTs
live — and which is also where a `LoadBlock`/`LoadTile` with a high base,
or a 32-bit load using the split-bank `high` range
(`fragment_lanes`' `SplitBanks` arm, `physical.rs:1886`), writes. And
Wall 3 is the *same* boundary from the other side: a CI source address
walking to exactly 0x800.

**Consolidated guess for the next card**: Walls 2, 3 and 4 are one defect
in TMEM high/low-half partitioning, seen three ways — a read addressing
an unloaded byte (W2), a read walking past 0x7ff (W3), and a write
landing in the TLUT region (W4). Investigate the write side
(`tmem/execute/`) and `linear_byte_address`/`fragment_lanes` together,
not the three refusals separately.

### Walls 2/3/4 root-cause candidate — partially-defined tail words

The existing WM2000 test fixture in
`crates/fn64-render-wgpu/src/tmem/read.rs:896-945` states the write side's
own rule, and it is the mechanism all three walls need:

- `WM2000_WORDS_PER_ROW = 5`, `WM2000_DEFINED_TAIL_BYTES = 2`,
  `WM2000_ROWS = 50`.
- The fixture comment: *"every row's last is only **partly** defined"* —
  the fifth word of each row defines **2 of its 8 bytes**.
- `PhysicalTmemPacketTransaction::stage_word_inner`
  (`tmem/physical.rs:648-657`) sets `valid[address] = false` for every
  lane whose `physical_lanes[lane]` is `None`. So a partly-defined tail
  word leaves **6 invalid bytes per row**, and it also *clears* validity
  those bytes may have had from an earlier load.

That last clause is the sharp edge: a partly-defined word does not merely
skip its undefined lanes, it **invalidates** them. So a later, narrower
load can punch holes in a region an earlier, wider load had fully
populated. Combined with `odd_row_exchange`'s XOR-4, a read can land in
one of those holes (Wall 2) even though its un-exchanged partner is
loaded — exactly the shape the existing
`wm2000_texrect_pixel_sixty_three_reproduces_the_production_invalid_byte`
test pins for byte `0x04c`.

**Guess (marked): the invalidate-on-undefined-lane rule at
`physical.rs:656` is the single highest-value thing for the next card to
scrutinize.** Whether real TMEM retains prior contents in lanes a load
does not cover is a hardware question this scout did not resolve.

### P3 — `FillExecutionError` (5) and `TargetError` (~10), read not reached
`crates/fn64-render-wgpu/src/targets/fill.rs:259` and
`crates/fn64-render-wgpu/src/targets/mod.rs:787`. WM2000's measured
packets carry **0 fills**, so these are the *least* likely to fire on the
attract path; listed for completeness. `TargetError` is reachable from
the texrect path too (`TexrectExecutionError::Target`).

### P4 — the commit path is a copy, not a refusal surface
`crates/fn64-abi/src/task_dispatch/rsp_commit.rs:1402`
(`copy_committed_guest_writes`) validates payload/write counts with
`assert_eq!` and then copies. It contains no capability refusals, so
once a texrect *executes*, its pixels reach guest RDRAM. Likewise
`present` (`production.rs:1939`) delegates straight to
`vi_scanout::scan_out_guest_rdram`. **The remaining risk between an
executed texrect and a visible frame is P2, not P4.**

---

## Wall 5 — TLUT entry at 0xa98 has only its low four bytes valid (`mask 0x0f`)

- **Error**: `execute_raw_dpc: render-wgpu/raw-dpc-execute backend error:
  texture rectangle texel fetch failed at pixel (12, 0): TLUT entry at
  0xa98 requires all eight valid bytes, found mask 0x0f`
- **Site**: `crates/fn64-render-wgpu/src/tmem/read.rs:584`
  (`read_canonical_tlut_entry` -> `IncompleteTlutEntry`).
- **Subsystem**: CPU texel read, TLUT entry validity.
- **Disabled**: the `if valid_mask != u8::MAX` arm (treat missing lanes
  as 0).
- **`mask 0x0f` is the finding.** Bytes 0-3 of the word are valid;
  bytes 4-7 are not. That is exactly the **4/4 split** of
  `PhysicalTmemError`'s `TmemTransferPhysicalWord::SplitBanks { low, high }`
  arm (`tmem/physical.rs:1886-1893`), where lanes 0..3 come from `low`
  and lanes 4..7 from `high`. **Something populated the low bank of this
  TLUT word and not the high bank.**
- **Guess: real defect, same family as Walls 2/3/4, now with the
  mechanism visible.** Wall 4 showed lane 3 of a TLUT word holding
  foreign data; Wall 5 shows lanes 4-7 of a TLUT word never written at
  all. Together they say the **TLUT high-half population is
  half-width** — consistent with a loader or plan that writes the low
  4-byte bank of each 8-byte word and skips the high bank.
- **Known divergence**: **new**.
- **Shape**: not one of the six. This is a write-side coverage gap, and
  it is the same one Wall 2 shows from the image side.

### Walls 4/5 mechanism — confirmed by reading the transfer geometry

`crates/fn64-render-wgpu/src/tmem/types.rs:606-616`, the `SplitBanks64`
arm of `project_tmem_transfer_word`:

```
let low  = destination * 8 + exchange;          // exchange is 0 or 4
low  .. low + 4
high: low + 2048 .. low + 2052
```

**`high = low + 2048` = `low + 0x800`.** So a split-bank (RGBA32-source)
load whose `low` fragment lands in the low half writes its **high**
fragment into the high half — the TLUT region. That is a concrete,
in-tree path by which a non-TLUT load deposits image bytes among TLUT
words, which is exactly what Wall 4's `[0100, 0100, 0100, 8f94]` looks
like, and the 4/4 boundary Wall 5's `mask 0x0f` reports.

**And the destination projection is a declared open frontier.**
`project_tlut_full_domain_word` (`types.rs:494-518`) wraps LoadTLUT
destination words against the **full 512-word domain** (`& 0x01ff`), and
its own doc says so plainly:

> *"This is an explicit RT64/reference-precedent parity policy, not a
> proven silicon fact ... Neither source defines what happens once
> `base + entry` advances past word 511. Real-hardware measurement of
> that overflow behavior remains the frontier this function does not
> close."*

So the *destination* of TLUT words past the end is an admitted guess in
the tree today. Whether WM2000's TLUT loads actually reach that overflow
is **unmeasured by this scout** — but the combination (a wrap policy that
is a guess, plus split-bank writes at +0x800, plus reads refusing
non-quadricated and partially-valid words) is a coherent explanation for
Walls 2 through 5 as **one** area rather than four cards.

---

## Cross-reference: the audit predicted Walls 3, 4 and 5

Checked after the fact against `docs/RT64-LANE-DIVERGENCES.md`. **This
materially revises the classifications above** — read this section over
the per-wall "Known divergence: new" lines, which were written before
the cross-reference.

| Wall | Refusal | Divergence row | Audit's verdict |
|---|---|---|---|
| 1 | GPU `tmem_sample` status 2 | (post-audit, D22-adjacent) | fixed once, now a second instance |
| 2 | `InvalidTexelByte` | mentioned at `RT64-LANE-DIVERGENCES.md:813` — reference has the **identical** refusal | **not** a divergence; both lanes refuse |
| 3 | `EnabledCiSourceOutsideLowHalf` | **D14** | **REFERENCE right on provenance**, silicon UNKNOWN |
| 4 | `NonCanonicalTlutEntry` | **D13** | **REFERENCE right** — wgpu can *write* a state it refuses to *read* |
| 5 | `IncompleteTlutEntry` | **D13** (second half) | **UNKNOWN** — same class, wgpu strictly stricter |

**Three of the five walls were already on the list.** The audit was
predictive, and its Tier placement for these ("plausible — TLUT is on
its path") is now upgraded to **measured: WM2000 hits all three**.

Two revisions to my own guesses above, on the audit's evidence:

- **Wall 4 is better explained by D13 than by my split-bank theory.**
  D13 records that wgpu's own
  `tmem/execute/load_tlut.rs:811-822` *deliberately* supports wrapping
  TLUT bases (base 511 across the bank), *"which produces exactly the
  unequal lanes `read.rs` then hard-refuses. **wgpu can write a state it
  will not read.**"* That is a simpler and better-sourced account of
  `[0100, 0100, 0100, 8f94]` than a foreign load clobbering lane 3. My
  split-bank reading stands as a *second* possible route, not the
  leading one.
- **Wall 3's severity is lower than I graded it.** D14 shows the
  reference lane imposes a low-half rule only for genuinely split-bank
  formats (RGBA32, YUV) and not for CI — so the wgpu constraint is
  invented, and the fix is to *drop* the constraint, not to add masking
  as I suggested. My "mask like RGBA32 does" suggestion would have been
  the wrong repair.

**The audit also says D13/D14 are read-side-only.** Both are refusals
wgpu applies that the reference does not; neither implicates the write
side. That weakens (does not kill) the "one TMEM partitioning defect"
consolidation I proposed for Walls 2-5. **Wall 2 is the one that stays
unexplained by the audit**, since the reference refuses it identically.

### Wall 2 is a known, owned bug — the audit names it

`docs/RT64-LANE-DIVERGENCES.md:813-816`, verbatim:

> **`InvalidTexelByte`** (`tmem/read.rs:309`). The reference has the
> identical uninitialized-TMEM trap (`gbi/state.rs:726-737` ...).
> Converged. *This is the current all-Rust blocker's error type and it is
> **not** a lane divergence — another lane owns the coverage bug behind
> it.*

So Wall 2 is (a) **not** a divergence — both lanes trap identically, the
refusal is correct — and (b) **already owned by another lane**. The
defect is the TMEM *coverage* behind it, not the trap. My "invalidate on
undefined lane" candidate (`physical.rs:656`) is a lead for that owner,
not a new card.

**Net: of the five walls reached, four are already accounted for in the
tree's own docs (W2 owned elsewhere, W3=D14, W4=D13, W5=D13), and W1 is
being fixed on `/private/tmp/fn64-gpubyte` right now.** The scout's value
is therefore mostly the *ordering* and the *measurement*, not the
discovery: it shows WM2000 hits D13 and D14 (previously "plausible,
unmeasured") and it shows how far the run gets.

---

## Wall 6 — triangle #0 needs a blend color that no SetBlendColor ever supplied · **NEW SUBSYSTEM**

First wall outside TMEM. Walls 2-5 were all texel reads; this is the
triangle draw-state admission gate.

- **Error**: `execute_raw_dpc: render-wgpu/raw-dpc-execute backend error:
  triangle draw state missing: triangle #0 (plan order) was visited
  before this plan's own first SetBlendColor command; a triangle draw
  cannot retrieve real state that was never admitted at its own stream
  position`
- **Site**: `crates/fn64-render-wgpu/src/production.rs:1041`
  (`PlanCollector`'s retrieval gate ->
  `MissingTriangleDrawState::NoBlendColor`).
- **Subsystem**: **plan admission / draw-state retrieval** (not TMEM, not
  the shader, not VI).
- **Trigger condition**: `other_mode.alpha_compare() ==
  AlphaCompare::Threshold` requires `current_blend_color.is_some()`
  (`production.rs:1035-1042`). Threshold alpha-compare reads the blend
  color's alpha as the threshold, so the requirement is substantively
  right.
- **Disabled**: the `.ok_or(...NoBlendColor...)?` — let the `Option` stay
  `None` and pass through (`blend_color` is already an `Option` on
  `RetrievedTriangleDraw`).

**Why this one is interesting, and why it is probably NOT simply a
missing-durable-state bug.** Durable cross-submission carry-in already
exists and is documented at `production.rs:864-874`: `PlanCollector::seeded`
takes `blend_color` from `WgpuBackend`'s durable `rdp_state`, and the
admission-time half seeds identically. So `None` here means the blend
color register was **never written by the guest at any point in the run**,
not that it was written in an earlier packet and forgotten.

- **Guess (marked)**: WM2000 selects `G_AC_THRESHOLD` without ever
  issuing `SetBlendColor`. On hardware the register would simply hold
  whatever it holds (power-on or stale), and the RDP would compare
  against that. **fn64 refuses rather than inventing a value, which is
  the house rule** — so this may be a *correct refusal* whose fix is a
  policy decision ("a never-written blend color reads as 0x00000000"),
  not a defect. That is the same shape as the fill-cycle-texrect case:
  the guard that was genuinely right.
- **Known divergence**: **new**. `NoBlendColor` is not in
  `RT64-LANE-DIVERGENCES.md`. Worth checking what
  `fn64-render-reference` does with an unset blend color under
  Threshold — I did not.

### Wall 6 verdict — measured against the reference lane: it IS a divergence

I checked the reference after writing the entry above, and it changes the
verdict.

`crates/fn64-render-reference/src/gbi/state.rs:227` declares
`blend_color: [u8; 4]` — **not** an `Option` — and `:387` initializes it
to `[0; 4]` at construction. `raster/blend.rs:242-260` reads
`state.blend_color[..]` unconditionally. **The reference lane has no
concept of an unset blend color**: it is a register with a defined
power-on value of zero, exactly as an RDP register behaves.

So:

- **Which lane is right: REFERENCE**, on the same reasoning D14 uses —
  one lane invents a precondition the other does not, and the invented
  one has no hardware citation behind it. An RDP colour register is not
  optional.
- **This is a new divergence row**, not present in
  `RT64-LANE-DIVERGENCES.md`. Suggested framing for the audit:
  *"`MissingTriangleDrawState::NoBlendColor`: an RDP register modelled as
  `Option` on one lane and as a zero-initialized register on the other."*
- **Revises my "probably a correct refusal" guess above.** It is not. The
  fix is to give the register its power-on value rather than to refuse,
  and the reference lane already shows the shape.
- Note the same question applies to `current_env_color`,
  `current_prim_color` and `current_fog_color`, which are `Option` on the
  wgpu side too (`production.rs:740-745`) — they are simply not
  *required* by any gate yet, so they have not aborted. **Guess**: same
  latent divergence, one gate away.

**Confirmed live, in the texrect path, adjacent to Wall 7.**
`crates/fn64-render-wgpu/src/targets/texrect.rs:1023-1031`
(`validate_combiner_program`) refuses with
`TexrectExecutionError::UnsetConstantRegister { Environment }` /
`{ Primitive }` when a combiner slot reads ENV or PRIM and the register
is `None`. That is Wall 6's exact shape — an RDP colour register modelled
as optional — sitting in the code path Wall 7 aborts in. **Predict: fix
Wall 7 (D4) alone and `UnsetConstantRegister` fires next**, since D4's
whole point is that the executor refuses inputs its own combiner
implements, and ENV/PRIM are two of the five inputs it *does* admit. The
two should be scoped as one card.

### Wall 6 latent siblings — confirmed for `fog_color`

`crates/fn64-render-reference/src/gbi/state.rs:228` declares
`fog_color: [u8; 4]` and `:388` zero-initializes it, the same shape as
`blend_color`. wgpu carries `current_fog_color: Option<Color4>`
(`production.rs:745`). So the *same* `Option`-vs-register modelling
mismatch exists for fog; it has not aborted only because no gate
requires it yet. Treat Wall 6 as one instance of a class, and fix the
class.

---

## Deepest reach — run 6, all six guards disabled

With Walls 1-6 all neutered the run **stopped aborting and started
rendering**. It passes the 50,000-step heartbeat and continues executing
at ~94% CPU with no further abort, which is qualitatively different from
every earlier run: previously each run aborted within seconds of the
heartbeat, and the CPU-side texel reader is now the hot path rather than
the refusal path.

This is the first evidence that WM2000's texrect strip **executes
end-to-end** on the all-Rust stack once the refusals are out of the way.
It is *not* evidence the pixels are correct — four of the six neuters
substitute a value the guard existed to prevent (zero bytes, lane 0 of a
non-canonical TLUT word, a missing blend colour). **A frame produced in
this configuration would be a wrong frame, not a validated one.** The
finding is about reachability, not correctness.

---

## The headline: a clean, bounded, abort-free 300-swap run

Run with `WM2000_STOP_AT_SWAP=300` on the same six-guard-disabled build:

```
[wm2000-boot] WM2000_STOP_AT_SWAP=300 satisfied (swap #300, step 52698,
              sim_time=957184887) -- stopping
[wm2000-boot] === BOOT SUMMARY ===
[wm2000-boot] virtual ticks run: 957184887
[wm2000-boot] VI swaps observed: 300
[wm2000-boot] gfx tasks submitted: 318
[wm2000-boot] audio tasks submitted: 562
[wm2000-boot] AI audio output: 561 buffers / 588432 samples (15427 nonzero)
[wm2000-boot] renderer: wgpu, graphics policy: HleOptimized
[wm2000-boot] last render error: None
[wm2000-boot] process exit prepared: threads=8 detached_coroutines=7
```

**Exit code 0. `last render error: None`. 300 VI swaps, 318 gfx tasks,
588,432 audio samples.** The all-Rust stack (`fn64-cpu-runtime` +
`fn64-render-wgpu`, no `--features rt64`, no C++ compiled) ran WM2000 to
a clean scripted stop without a single render refusal.

**The essential caveat, restated.** Four of the six disabled guards
substitute a value the guard existed to prevent — unwritten TMEM bytes
read as 0, non-quadricated TLUT words take lane 0, partially-valid TLUT
words take what's there, a missing blend colour passes as `None`. **The
pixels in this run are not trustworthy.** What the run establishes is
*reachability*: nothing structural stands between the all-Rust stack and
a sustained frame loop except these six refusals and whatever correctness
work each one really needs.

The unbounded run (`run6.log`) independently confirms it, reaching
**150,000 steps / 825 VI swaps / 1,889 gfx tasks** and still climbing
when observed.

---

## A CAPTURED FRAME WITH REAL CONTENT

The dump-enabled run wrote **298 framebuffer PNGs** (320x240 RGBA8) and
reported `frame images dumped: 298`. Scanning them for distinct byte
values:

- Frames 3-204: uniform (grey `0xbd`, then black) — 2-3 distinct values.
- **Frames 205-247: real content**, distinct-value count ramping
  9 -> 11 -> 13 -> 15 -> 17 -> 18 -> 21, holding ~20 for thirty frames, then
  fading 15 -> 18 and back to uniform. That ramp-hold-fade is a **fade-in**,
  consistent with the attract loop's known content -> white -> content
  behaviour.
- Frames 248-300: uniform black again.

**Swap 220 evidence**: the PNG was retained in operator scratch only. It is
not committed because it contains game output, and no digest is cited because
no repository test can gate an external artifact.

**What it shows.** Structured composed geometry: a white band across the
top third, a large black field, and **two blue textured blocks in the
lower half** — the HUD/sprite strip the census describes. So the texrect
path is placing real texels at real coordinates in guest RDRAM, and VI
scanout is reading them back.

**What is wrong with it.** The whole lower two-thirds is rendered as
**alternating black/white horizontal scanlines** — one-pixel-period
striping across the full width. That is the visual signature of a
**row-parity / interleave defect**: every other row is being sourced from
the wrong place. This is precisely the class of bug
`/private/tmp/fn64-gpubyte` is working on (the WGSL sampler's hardcoded
row parity), and the CPU-side sibling of it
(`targets/texrect.rs:1237`'s `first_row_parity`) was fixed earlier today.
Seeing it *in the output image* rather than as an abort is new
information: **the parity defect survives into the composed frame, it is
not only a refusal trigger.**

The blue blocks also show vertical banding at roughly a 4-pixel period,
which would be consistent with the 4-bit/8-byte-lane addressing the TMEM
walls above all point at.

**Caveat, again**: six guards are disabled and four of them substitute
invented values. This frame is evidence that the *pipeline* runs
end-to-end and that the parity defect is visible in output. It is not
evidence any pixel is correct.

### Frame 244 confirms it — and the top band is a fade, not a bug

Swap 244 (near the end of the content window) shows the same composition
with the top band having faded from white to **mid-grey**, the lower
field still fully striped, and the two blue blocks unchanged. The
top-band luminance changing while the geometry stays fixed is the attract
loop's documented fade, exactly as the card predicted ("uniform white is
a fade, not a stall"). So:

- the **fade** is being rendered correctly (a smooth luminance ramp
  across ~40 frames),
- the **geometry** is being placed correctly (band, field, two blocks in
  the lower half, stable across the window),
- the **texel rows** are wrong (one-pixel-period striping).

That is a useful separation: whatever is broken is in texel addressing,
not in composition, placement, or VI.

---

## Wall 7 — texrect combiner slot C selects `ShadeAlpha` · **1,887 VI SWAPS IN**

Reached at **350,000 steps / 1,887 VI swaps / 5,125 gfx tasks** — deep
into the attract loop, far past the captured-frame window.

- **Error**: `execute_raw_dpc: render-wgpu/raw-dpc-execute backend error:
  execute_texture_rectangle evaluates only
  TEXEL0/PRIMITIVE/ENVIRONMENT/ONE/ZERO color inputs (plus COMBINED in a
  two-cycle program's second cycle); slot C selects ShadeAlpha`
- **Site**: `crates/fn64-render-wgpu/src/targets/texrect.rs`,
  `TexrectExecutionError::UnsupportedColorInput`.
- **Subsystem**: texrect combiner evaluation.
- **Known divergence**: **D4** — *"Combiner inputs the executor refuses
  but its own combiner implements · REACHES WM2000: yes, texrects are its
  entire title path"*. **Predicted by the audit and now measured.**
  D4 was already scored a wgpu-side defect: the crate's own
  `combiner.rs` implements the input the executor refuses.
- **This is predicted-wall P1 firing**, first entry
  (`UnsupportedColorInput`).
- **Guess: real defect**, per D4's own adjudication, which I did not
  re-derive.

---

## How many walls remain between here and a frame?

**A frame is already reached** (swap 220 above), so the honest form of the
question is: *how many walls remain between here and a frame produced
without disabled guards?*

**Answer: at least seven, and the count is bounded below, not above.**
The scout stopped finding walls only where it stopped running, not where
the walls stopped.

Ordered, with owner:

1. **W1** GPU `tmem_sample` status 2 — *in flight on
   `/private/tmp/fn64-gpubyte`*
2. **W2** `InvalidTexelByte` / TMEM coverage — *owned by another lane per
   `RT64-LANE-DIVERGENCES.md:813`*; correct refusal, the bug is behind it
3. **W3** `EnabledCiSourceOutsideLowHalf` = **D14** — invented
   constraint, drop it (do NOT mask, see the cross-reference section)
4. **W4** `NonCanonicalTlutEntry` = **D13** — wgpu writes a state it
   refuses to read
5. **W5** `IncompleteTlutEntry` = **D13** second half — UNKNOWN,
   wgpu strictly stricter than reference
6. **W6** `NoBlendColor` — **new divergence**; reference models the RDP
   colour registers as zero-initialized `[u8;4]`, wgpu as `Option`.
   Class bug: `fog_color` confirmed to share it
7. **W7** `UnsupportedColorInput{ShadeAlpha}` = **D4** — the crate's own
   `combiner.rs` implements what the executor refuses

Beyond those, **P1 alone lists 21 texrect refusal variants** and only one
has fired so far; D5, D7 and the fill-cycle D3 row all sit in the same
executor. **Nobody should read this list as "seven and done."** The rate
of discovery did not fall off — W7 appeared 1,887 VI swaps in, which
means later walls are gated behind *game state*, not behind code paths,
and a scout that ran twice as long would likely find more.

### What is NOT in the way

Measured or read, not assumed:

- **VI scanout.** WM2000's first content field latches AA mode 2 with
  divot/gamma/fade/repeat-line clear; both filters it selects are
  implemented. D1's Tier-A "aborts the first frame" placement looks
  stale (see P2).
- **The guest commit path.** `copy_committed_guest_writes` is a
  validated copy with no capability refusals (P4).
- **Composition, placement and the fade.** Demonstrated correct by the
  captured frames.
- **Boot, threading, audio.** 8 threads, 588,432 audio samples,
  `thread 0 dead: true` as expected, clean scripted exit.

### The one structural item that no wall list covers

**S1: the GPU raster path still has no RDRAM writeback** (see the S1
section). Every frame captured here was composed by the **CPU** texrect
path. Raw triangles raster into `triangle_draw_output`, which `present`
refuses to scan out by name and which nothing copies to guest memory. So
clearing all seven walls yields a *texrect* frame; 3D geometry needs S1
closed as well, and that is a design change rather than a refusal
widening.

---

## Reproducing this

Scratch branch `scout/wall-preview` off `201dfb92`. The six/seven guard
neuters are in one clearly-labelled commit
(`SCRATCH ONLY -- DO NOT MERGE`), all hunks marked `SCOUT HACK`.

```
git -C <fn64> worktree add /private/tmp/fn64-scout scout/wall-preview
cd ~/Code/recomps/wm2000
FN64=/private/tmp/fn64-scout ./packages/wm2000-boot/rs/run-rs-lane.sh
```

For the frame capture, drop `WM2000_NO_TRACE` (it gates the PNG dumps
too) and bound the run:

```
env ROM=$AKI/games/NWXE/wm2000.z64 FN64_ABSENT_N64DD=1 FN64_NO_AUDIO=1 \
    FN64_RENDER=wgpu WM2000_STOP_AT_SWAP=300 WM2000_MAX_STEPS=2000000 \
    ./packages/wm2000-boot/rs/target/release/wm2000-boot
```

Frames land at `/tmp/fn64-fb-<swap>.png`. Content appears in swaps
**205-247**; everything else in the window is uniform.

---

## Run 7 (Wall 7 also disabled) — in flight when this report was written

With the seventh guard (`UnsupportedColorInput`) also neutered, the run
was still going when the scout wrapped up, having reached:

```
[wm2000-boot] progress: steps=300000 sim_time=5105023577 vi_swaps=1626 gfx_tasks=4332 audio_tasks=2993
```

**It then passed Wall 7's abort point cleanly.** Run 6 aborted at
`steps=350000 vi_swaps=1887 gfx_tasks=5125`; run 7 printed **the
identical heartbeat and kept going**:

```
[wm2000-boot] progress: steps=350000 sim_time=5921204353 vi_swaps=1887 gfx_tasks=5125 audio_tasks=3471
```

Same step count, same swap count, same gfx-task count, no panic. That is
a clean confirmation that Wall 7 (D4, `UnsupportedColorInput{ShadeAlpha}`)
was the sole blocker at that point and that nothing else fires with it.

It kept running well past that point, last observed at:

```
[wm2000-boot] progress: steps=400000 sim_time=6742069053 vi_swaps=2149 gfx_tasks=5967 audio_tasks=3952
```

**2,149 VI swaps — over half the attract loop's ~3,990 — with all seven
guards disabled and no eighth wall reached.**

No Wall 8 was observed before the scout wrapped up. The predicted next
refusal remains `UnsetConstantRegister` (ENV/PRIM), eight lines below
Wall 7 in the same function; it did not fire in the 262 swaps between
Wall 7's old abort point and where observation stopped, so either WM2000
sets those registers or the combiner programs past that point do not read
them. **Do not read that as "there is no
Wall 8"** — it means only that the scout stopped before the run did. The
predicted next refusal is `UnsetConstantRegister` (see the Wall 6 latent
siblings section), which sits eight lines below Wall 7 in the same
function.
