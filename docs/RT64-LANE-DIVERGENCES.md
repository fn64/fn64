# Lane divergences: `fn64-render-reference` vs `fn64-render-wgpu`

Every pinned disagreement between fn64's two renderer lanes, with which lane
the evidence favors and whether WM2000's measured path reaches it.

This audit exists because twice in one day `fn64-render-wgpu` aborted the
all-Rust WM2000 run on a hardware rule `fn64-render-reference` had already
implemented correctly, with the answer sitting in a wgpu-lane doc comment the
whole time. Grepping for the pattern is cheaper than rediscovering each one at
an abort.

Measured read-only at `4371d57a`. Nothing here was changed; every row cites
file and line on both sides. Where a lane could not be adjudicated the row says
**UNKNOWN** rather than guessing.

Companion docs: [`RT64-WM2000-REMAINING.md`](RT64-WM2000-REMAINING.md) (its §3
V1/V4/V5/V7 rows are the predecessor to this table),
[`VI-FILTERS.md`](VI-FILTERS.md),
[`RT64-WM2000-CENSUS.md`](RT64-WM2000-CENSUS.md).

---

## 0. Since this audit was taken

**Two further divergences, both found at an abort rather than by grepping, and
both already fixed.** Not renumbered into the table below, which stays as
measured at `4371d57a`.

### D22 — GPU triangle sampler refused a non-RGBA16 tile under an enabled TLUT · **REACHED WM2000: FIRST TEXTURED TRIANGLE**

- **wgpu** `crates/fn64-render-wgpu/src/shaders/tmem_sample.wgsl`,
  `sample_committed_rgba16_three_nearest`'s format gate, surfacing as
  `TMEM_SAMPLE_STATUS_UNSUPPORTED_FORMAT` (4) and aborting the all-Rust stack
  at `crates/fn64-abi/src/task_dispatch/rsp_commit.rs:1202`.
- **reference** implements the palettized path; so, since `4c412a96`, does
  wgpu's own CPU reader (`tmem/texel.rs`'s `resolve_indexed_texel`).
- **Disagreement.** The shader consulted `tile.format` unconditionally. Under
  `tlut_en` the RDP sources the texel from a palette and the tile format is
  ignored (n64brew `Reality_Display_Processor/Pipeline`; RT64's `sampleTMEM`,
  `TextureDecoder.hlsli:149-208`, branches on `usesTlut` before any format
  dispatch and never reads `fmt` in that arm).
- **Which lane was right: REFERENCE.** This is §1's structural cause 2
  (*wiring gaps described as capability gaps*) in its purest form: a sibling
  module in the same crate — the CPU reader the texrect path already uses —
  had implemented the rule hours earlier. The shader could not even ask the
  question, because `TileBindingParams` carried no `lut_mode` and no
  `palette`.
- **WM2000 reach.** Measured at the abort, not inferred: `tile format code 3`
  (`IntensityAlpha`), `pixel-size code 0` (`Bits4`), `TLUT-mode code 2`
  (`Rgba16`).
- **Status: FIXED.** `lut_mode` is consulted before `format`; 4/8/16-bit
  texels palettize (4-bit through the tile's `palette` field); 32-bit stays
  refused on both arms, matching `4c412a96`, which deliberately did not widen
  there. Pinned by five tests, four adapter-gated; ten of ten shader mutants
  killed. The run then advanced to a different refusal
  (`NoCompletedLoads`), one layer up in raw-DPC plan admission -- D23 below.

**Method note for the next lane.** The abort named only a status code, which
sent an earlier reader to the CPU-side tile to guess the shape.
`WgpuRawDpcExecutionError::TmemSampleFailed` now carries the triangle index
and the tile's format/size/TLUT codes, so the shape is measured at the abort.

### D23 — raw-DPC execution refused a sync-only packet · **REACHED WM2000: FOURTH ABORT**

- **wgpu** `crates/fn64-render-wgpu/src/production.rs`,
  `stage_and_report`'s no-completed-transaction arm, surfacing as
  `WgpuRawDpcExecutionError::NoCompletedLoads` and aborting the all-Rust
  stack at `crates/fn64-abi/src/task_dispatch/rsp_commit.rs:1202`.
- **The refused packet, measured — not inferred.** Instrumented at the
  refusal site and run on the real ROM through the all-Rust lane
  (`FN64_RECOMP=rs`, `FN64_RENDER=wgpu`): **one wire command**,
  `wire_opcode = 0xE9` (`G_RDPFULLSYNC`), raw words
  `[0xE9000000, 0x07000000]`; **0 loads, 0 triangles, 0 texrects, 0 fills**;
  one `ResourceAccess`, `Read`/`CommandDecode` over the 8 `RspDmem` bytes of
  the sync command itself; site `dp_slot_reserved: true`,
  `interrupt_after: Clear`.
- **Disagreement — internal to wgpu, before any lane comparison.** The
  `Display` string said "zero TMEM loads"; the doc comment said "zero loads
  AND zero admitted triangles"; the code checked triangles only. All three
  descriptions of one guard, and the packet satisfied every one of them
  while still being a legitimate command.
- **Which lane was right: NEITHER — the guard was WRONG on its own terms.**
  `PlanCollector`'s own `FullSyncSite` arm already states the semantics:
  the site is *"collected, not executed ... retained so the executed plan
  still accounts for every command the plan carried"*, and dropping it
  *"would be wrong in the other direction"*. `RdpFullSyncSite`'s doc adds
  that a sync *"reads and writes no resource"*. The refusal contradicted
  two doc comments in its own crate. "Zero raster work" and "nothing to do"
  are not the same claim.
- **Status: FIXED.** A sync-only plan now completes via
  `StagedOutcome::NoPhysicalSuccessor` (renamed from `TriangleOnly`, which
  named the wrong one of its now-two producers) through
  `complete_execution_preserving_physical`. **This is not a weakening**: that
  destination builds its own explicitly empty write list and rechecks it
  against the packet's real journal via `BackendEffectReport::try_new`, so a
  write-bearing packet routed there is still rejected with
  `EffectCountMismatch` — the zero-write property is *proved* at the
  destination, not assumed at the branch. The refusal itself is kept and
  narrowed to "no load, no triangle, AND no sync", pinned by its own test
  after the over-widening mutant was found to survive the suite. Three of
  three mutants killed. The run now advances to
  `MixedTexrectAndRawTrianglePacket`.

**Method note for the next lane.** Three descriptions of one guard disagreed,
and the code was the least accurate of the three. When an error message and
its doc comment differ, measure the packet before believing either — the
instrumentation that answered this took one run and ruled out four candidate
shapes at once.

---

## 1. Headline

**Twenty-one pinned divergences. Fifteen are wgpu-side defects — the reference
lane already implements the behavior, in five cases citing the very source the
wgpu side quotes and then declines to act on. One of the fifteen aborts
WM2000's first frame, and five more sit directly in its measured texrect
path.**

| Verdict | Count | Rows |
|---|---|---|
| **Reference-correct** (wgpu refuses, reference implements) | 15 | D1–D9, D11–D14, D16, D20 |
| **wgpu-correct** (wgpu right, reference over-claims) | 0 | — |
| **UNKNOWN** (no evidence in this repo settles it) | 6 | D10, D15, D17, D18, D19, D21 |

D20 is scored reference-correct on the narrow ground that the *inconsistency*
is a defect regardless of which table wins; which table wins is D19 and is
UNKNOWN.

Three structural causes account for eleven of the fifteen. This matters for
sequencing: they are not eleven independent fixes.

1. **One missing datum.** `fn64-render-reference` keeps a 195-line per-pixel
   coverage sidecar
   (`crates/fn64-render-reference/src/backend/hidden_bits.rs`,
   `RdramHiddenBits`) that `fn64-render-wgpu` does not maintain. Every wgpu
   refusal naming "coverage this backend does not track" is downstream of that
   one absence: **D1, D5, D8, D9.**
2. **Wiring gaps described as capability gaps.** `fn64-render-wgpu`'s own
   `combiner.rs`, `blend.rs`, `coverage.rs`, and `alpha_compare.rs` already
   implement behaviors that `targets/texrect.rs` refuses as unimplementable.
   Four refusal doc comments are factually contradicted by sibling modules in
   the same crate: **D2, D4, D5, D7.**
3. **Cite-then-decline.** A doc comment names n64brew, RT64's
   `TextureDecoder.hlsli`, the SGI RDP Command Summary, or the reference lane,
   states what the source establishes, and then declares it out of scope:
   **D3, D6, D11, D14, D17.**

A fourth pattern appears twice and deserves its own name: **wgpu refusing a
state wgpu itself can produce.** Its TLUT loader deliberately supports wrapping
bases whose result its TLUT reader then rejects (D13), and its TMEM reader
handles 4-bit texels its loader will not load (D12).

---

## 2. The table

Ranked by whether WM2000's measured path reaches it: **Tier A** is proven
reachable, **Tier B** is plausibly on the path but unmeasured, **Tier C** is
unreachable today or blocked behind another row.

---

### Tier A — proven to sit in WM2000's measured path

#### D1 — VI silhouette antialiasing (AA modes 0/1) · **REACHES WM2000: FIRST FRAME**

- **wgpu** `crates/fn64-render-wgpu/src/vi_scanout.rs:72`
  (`ViScanoutRefusal::SilhouetteAntialias`), raised at
  `vi_scanout.rs:329-331`.
- **reference** `crates/fn64-render-reference/src/vi.rs:83-103` and `259-296`
  (`filter_scanout`, `CoverageAaNeighborhood`, `estimate_coverage_background`),
  US 5,742,277 Figure 11.
- **Disagreement.** wgpu refuses AA modes 0 and 1 outright: "needs per-pixel
  coverage, which guest RDRAM RGBA16 carries in its low bit and hidden bits --
  state this backend does not track." The reference implements the full
  estimator over exactly that data.
- **Which lane is right: REFERENCE.** Three independent lanes implement it —
  the reference (above), the RT64 native adapter
  (`docs/rt64-port-authority.json:47`, mechanism `vi-silhouette-aa:v1`), and
  the certification example
  `crates/fn64-certification/examples/rt64_vi_aa_selector_behavior.rs`. wgpu is
  the only one of the three refusing. The refusal's stated reason is accurate
  about *why* (no sidecar) but the conclusion — refuse — is a lane gap, not a
  hardware rule.
- **WM2000 reach.** Measured, not inferred. The wgpu run's *first VI present*
  aborts here: `VI STATUS selects coverage silhouette antialiasing (AA mode 0
  or 1); this scanout implements only AA mode 3`
  ([`RT64-WM2000-REMAINING.md:25`](RT64-WM2000-REMAINING.md)). This is the
  highest-priority row in the table.
#### D2 — Two-cycle texture rectangles · **REACHES WM2000: yes, by census**

- **wgpu** `crates/fn64-render-wgpu/src/targets/texrect.rs:369`
  (`UnsupportedCycleType`), raised at
  `texrect.rs:1158-1163` in `admitted_cycle_evaluates_combiner`.
- **reference** `crates/fn64-render-reference/src/backend/validate.rs:131`
  admits `TwoCycle`; `crates/fn64-render-reference/src/raster/draw.rs:438-441`
  asserts `OneCycle | TwoCycle`;
  `crates/fn64-render-reference/src/raster/combiner.rs:65` runs both cycles.
- **Disagreement.** wgpu's variant doc says two-cycle "needs the `Combined`
  carry and a second texel, neither of which this executor supplies."
  **That reason is factually wrong about its own crate.**
  `crates/fn64-render-wgpu/src/combiner.rs:1021` is a public
  `run_two_cycle`; the cross-cycle carry is modeled by
  `CyclePass::SecondOfTwoCycles` (`combiner.rs:815`, `carries_wrap` at
  `combiner.rs:829`); `Texel1` inputs exist at `combiner.rs:576` and `:633`.
  The capability is present and unwired.
- **Which lane is right: REFERENCE.** The refusal's stated cause is
  contradicted by a sibling module in the same crate. **The refusal site says
  so itself**: `texrect.rs:1153-1155` records "Measured, not stylistic: while
  this match was inline, widening it to admit two-cycle left the entire suite
  green."
- **WM2000 reach.** The census measured **0 two-cycle texrects of 2,520** in
  the boot-through-attract window
  ([`RT64-WM2000-CYCLE-MODES.md`](RT64-WM2000-CYCLE-MODES.md) §1), so this is
  Tier A on the *texrect path* rather than on a proven two-cycle draw. Read the
  zero correctly: it means "not seen in boot/logo/attract," never "does not
  occur." Gameplay has never been reached on either lane.
- **RESOLVED (`6c0dc19a`).** Two-cycle now evaluates through
  `combiner::run_two_cycle`. Validation follows the reference rather than the
  old constant set: `TexrectShading::validate_combiner_program` checks every
  bitfield slice the cycle mode actually evaluates and admits `COMBINED` only
  in two-cycle's second slice (`validate.rs:476-478`'s rule); `TEXEL1` stays
  refused in both slices, because a rectangle binds one tile
  (`validate.rs:479-483`, the reference's own reason). The audit's warning was
  acted on: `two_cycle_carries_the_accumulator_one_cycle_cannot` runs a program
  whose cycle 0 is `(0-0)*0 + Primitive` and whose cycle 1 is
  `(0-0)*0 + Combined`, so two-cycle must give the primitive colour and the
  same program as one-cycle must give transparent black. Four mutants killed.

#### D3 — Fill-cycle texture rectangles · **REACHES WM2000: unmeasured; broke a sibling ROM**

- **wgpu** `crates/fn64-render-wgpu/src/targets/texrect.rs:369`
  (`UnsupportedCycleType`), same site as D2.
- **reference** `crates/fn64-render-reference/src/backend/validate.rs:147`
  (admits Fill, checking only the genuine fill-cycle blender hazard) and
  `crates/fn64-render-reference/src/backend/imp.rs:911-919`, which executes it
  as `draw_fill_rectangle(&rectangle.as_fill_cycle_rectangle(), target)`.
- **Disagreement.** wgpu refuses Fill-cycle texrect because it "samples no
  texture at all." The reference agrees sampling is bypassed and draws the
  rectangle anyway, from the fill color register.
- **Which lane is right: REFERENCE, with a primary source and a regression
  witness.** The reference's comment
  (`validate.rs:133-140`) quotes **n64brew's RDP command table, Texture
  Rectangle section**, verbatim: *"In FILL mode this behaves identically to
  Fill Rectangle, the texturing properties are ignored."* It further records
  that refusing this **aborted a real WCW/nWo Revenge frame** — a shipped
  AKI-engine sibling of WM2000. wgpu's variant doc
  (`texrect.rs:365-368`) offers only a WM2000 measurement showing zero Fill
  texrects in one window: an absence-of-evidence argument that does not
  contradict spec text.
- **WM2000 reach.** UNKNOWN for WM2000 itself; **proven** for its engine
  sibling. Listed in Tier A because the failure mode is already witnessed on
  the same engine.
- **NOT LANDED, and it is NOT one match arm** (checked at `6c0dc19a`, pinned
  by `the_texrect_and_fill_rectangle_rules_disagree_by_a_pixel_on_every_axis`
  and `the_fill_rule_refuses_a_fractional_edge_the_texrect_rule_rounds`). The
  verdict above stands — the reference is right and this is a lane gap — but
  the estimate of the fix does not. Widening `admitted_cycle_evaluation` to
  admit `Fill` would draw the wrong rectangle, silently. Three things block it,
  each in a different module:
  1. **The two rectangle rules disagree by one pixel on every axis.** A texrect
     reaches the executor as an already-resolved `RectViewportPixels`, built by
     `raw_dpc/texture_rectangle.rs`'s port of RT64's `FixedRect`:
     `(coord + 3) >> 2` at both ends, **half-open**. A fill rectangle's rule is
     `targets/fill.rs`'s `resolve_fill_pixel_rectangle`: `coord >> 2` at both
     ends, **inclusive** (`width = x1 - x0 + 1`). On wire `(0, 0, 1276, 956)`
     the first gives 319x239 and the second 320x240. On `ulx = 2` the first
     rounds down and the second refuses `FractionalEdge`.
  2. **`FillColor` is not on this path.**
     `raw_dpc::triangle_draw_data::RetrievedTriangleDraw` snapshots
     `blend_color`, `env_color`, `prim_color` and `fog_color` per triangle. It
     does not snapshot the fill colour, because no triangle-sourced command has
     ever read it — and a Fill-cycle texrect reads nothing else.
  3. **The fill-cycle blender hazard must run.** It is a property of the cycle,
     not the command (`backend/validate.rs:152-161`), and
     `targets/fill.rs`'s `require_safe_fill_cycle_bypass` is this crate's
     equivalent.

  The real shape: carry the raw wire rectangle alongside the viewport,
  snapshot `FillColor` on the triangle path, and route the command to
  `execute_fill_rectangle` rather than through the texrect executor at all —
  which is exactly what the reference does. Three modules, not one arm. The
  refusal's own doc comment now carries this, so the next lane meets it before
  an abort rather than after.

#### D4 — Combiner inputs the executor refuses but its own combiner implements · **REACHES WM2000: yes, texrects are its entire title path**

- **wgpu** `crates/fn64-render-wgpu/src/targets/texrect.rs:376` / `:381`
  (`UnsupportedColorInput` / `UnsupportedAlphaInput`). The admitted set is
  `ADMITTED_COLOR_INPUTS` / `ADMITTED_ALPHA_INPUTS`
  (`texrect.rs:750-766`) — only `Texel0`, `Primitive`, `Environment`, `One`,
  `Zero`. Raised at `texrect.rs:821` and `:836`.
- **reference** `crates/fn64-render-reference/src/raster/combiner.rs:119-147`
  (`color_input`, all 21 `ColorSource` variants) and `:149-162` (`alpha_input`,
  all 10). Rect-specific gating at
  `crates/fn64-render-reference/src/backend/validate.rs:476-489`.
- **Disagreement.** The reference refuses **only** `Shade`/`ShadeAlpha`,
  `Combined` in cycle 0, and `Texel1` with no decoded tile+1. It implements
  `Texel1`, `Texel0Alpha`, `PrimitiveAlpha`, `EnvironmentAlpha`,
  `LodFraction`, `PrimLodFrac`, `K4`, `K5`, `KeyCenter`, `KeyScale`, `Noise`,
  and cycle-1 `Combined`. wgpu refuses all twelve — **and
  `crates/fn64-render-wgpu/src/combiner.rs:574-641` implements every one of
  them.**
- **Which lane is right: REFERENCE**, for all twelve. `Shade`/`ShadeAlpha` is
  excluded from this row and is genuine agreement (see §3).
- **WM2000 reach.** WM2000's title path is texrects — 2,520 in the measured
  window, all one-cycle
  ([`RT64-WM2000-CYCLE-MODES.md`](RT64-WM2000-CYCLE-MODES.md) §1). The census
  records only three distinct combiner programs with `Shade`/`Texel1`/
  `Combined` unread, so the *measured* programs stay inside the admitted set.
  Any program outside it aborts, and the window has never reached gameplay.

#### D5 — Blender `blend_enabled` derivation · **REACHES WM2000: same texrect path as D4**

- **wgpu** `crates/fn64-render-wgpu/src/targets/texrect.rs:498`
  (`BlendEnabledNotDerivable`), raised at `texrect.rs:1583`.
- **reference** `crates/fn64-render-reference/src/raster/coverage.rs:68-69` —
  `blend_enabled = force_blend() || (antialias_enabled() && !wraps)`, with
  `wraps` read from real memory coverage.
- **Disagreement.** wgpu refuses the `FORCE_BL`-clear + `AA_EN`-set case as
  underivable. The reference computes it exactly.
  `crates/fn64-render-wgpu/src/coverage.rs:148` computes the identical
  expression already; it is gated only on the missing coverage source (D1's
  sidecar).
- **Which lane is right: REFERENCE — and wgpu's own doc comment cites
  `fn64-render-reference/src/raster/coverage.rs:68-69` as the authority, then
  declines to follow it.**
- **WM2000 reach.** Same texrect path as D4; the specific mode bits are
  unmeasured because the census does not decode `G_RDPSETOTHERMODE` payloads.

#### D6 — RGBA4 / RGBA8 aliasing to I4 / I8 · **REACHES WM2000: unmeasured; cite-then-decline**

- **wgpu** `crates/fn64-render-wgpu/src/tmem/texel.rs:510`
  (`DirectTexelDecodeError::UnsupportedPair`), reached by `(Rgba, Bits4)` and
  `(Rgba, Bits8)`. Pinned at `crates/fn64-render-wgpu/src/tmem/read.rs:797-806`.
- **reference** `crates/fn64-render-reference/src/gbi/state.rs:962-963` and
  `crates/fn64-render-reference/src/gbi/tmem.rs:459-465`.
- **Disagreement.** wgpu treats RGBA at 4 and 8 bits as an unsupported pair.
  The reference aliases both to the intensity decoders, exactly as hardware
  does.
- **Which lane is right: REFERENCE, decisively. This is the sharpest
  cite-then-decline in the audit.** wgpu's module header
  (`tmem/texel.rs:41-49`) names the RT64 lines, states what they establish, and
  then declines: *"`sampleTMEM4b`/`sampleTMEM8b`/… select `I*ToFloat4` for
  `G_IM_FMT_I` and reuse it for `G_IM_FMT_RGBA` at 4/8 bit, citing hardware
  observation rather than a distinct real format; that RGBA/I aliasing at 4/8
  bit is out of scope here."* Verified against upstream in this audit: RT64's
  `sampleTMEM4b`
  (`/Users/jer/Code/no-mercy-recompiled/third_party/rt64/src/shaders/TextureDecoder.hlsli:51-52`)
  falls `G_IM_FMT_RGBA` through to `sampleTMEMI4` under its own comment
  *"Not a real format. Replicated by observing hardware behavior."* The
  reference cites the same lines **and** an observed OoT 250-swap C-boot trace
  that exercises the pair (`gbi/tmem.rs:461-463`), then implements it.
- **WM2000 reach.** UNKNOWN. The census records no tile-format operand data.

#### D7 — Alpha-dither refused by citing the *other* module's disagreement · **REACHES WM2000: same texrect path**

- **wgpu** `crates/fn64-render-wgpu/src/targets/texrect.rs:460`
  (`OrderedDitherAuthorityUnsettled`), raised at `texrect.rs:1424` for the
  **alpha-dither** stage.
- **reference** `crates/fn64-render-reference/src/raster/blend.rs:82-95`
  (`apply_alpha_dither` and its substitution rule).
- **Disagreement.** The refusal declines the alpha-dither stage on the grounds
  that the RT64 and reference Bayer tables disagree — a disagreement that lives
  in `rgb_dither.rs` (the *RGB* stage). But
  `crates/fn64-render-wgpu/src/alpha_compare.rs:174-176` holds a second Bayer
  table that is **byte-identical to the reference's**, and `apply_alpha_dither`
  (`alpha_compare.rs:204-227`) is a declared literal port of the reference. For
  the stage actually being refused, wgpu already agrees with the reference
  cell-for-cell.
- **Which lane is right: REFERENCE.** The cited authority conflict does not
  apply to the stage it is being used to refuse. This is distinct from D17,
  which is the genuine unresolved table question.
- **WM2000 reach.** Same texrect path as D4.


---

### Tier B — plausibly on WM2000's path, reach unmeasured

#### D8 — Blender `B = FramebufferAlpha` / destination coverage · **REACHES WM2000: unmeasured; same root cause as D1**

- **wgpu** `crates/fn64-render-wgpu/src/targets/texrect.rs:470`
  (`DestinationCoverageUnavailable`) and `:486`
  (`UnsupportedBlendFramebufferAlpha`).
- **reference** `crates/fn64-render-reference/src/backend/hidden_bits.rs:24-195`
  (`RdramHiddenBits`, `read_rdram_hidden_bits`, `write_rdram_hidden_bits`).
- **Disagreement.** The destination coverage count is 3 bits: RGBA16's visible
  LSB plus a 2-bit hidden sidecar. wgpu maintains no sidecar, so it can recover
  only 1 of 3 bits and refuses by name. The reference maintains the sidecar and
  resolves the term.
- **Which lane is right: REFERENCE.** wgpu's own doc comment concedes the point
  ("the oracle does, as `RdramHiddenBits`"). Refusing rather than guessing from
  one third of the bits is the *correct local* call; the divergence is that the
  sidecar was never built on the wgpu side.
- **WM2000 reach.** UNKNOWN. The census counts opcodes and does not decode
  `G_RDPSETOTHERMODE` payload bits, so no evidence shows WM2000 selecting a
  coverage-consuming blend mode. Absence in the census window is not absence.
#### D9 — VI divot filter · **REACHES WM2000: unmeasured**

- **wgpu** `crates/fn64-render-wgpu/src/vi_scanout.rs:82`
  (`ViScanoutRefusal::Divot`), raised at `vi_scanout.rs:337`.
- **reference** `crates/fn64-render-reference/src/vi.rs:104-106` and `542-566`
  (`apply_divot`), US 6,166,748.
- **Disagreement.** VI STATUS bit 4 selects a three-tap horizontal median over
  post-filter samples. wgpu refuses; the reference computes the componentwise
  median of the left/center/right samples, gated on the neighborhood not being
  uniformly full-coverage.
- **Which lane is right: REFERENCE.** The reference cites the patent, the RT64
  native lane implements the same mechanism (`vi-divot:v1`,
  `docs/rt64-port-authority.json:47`), and the certification gate measures it
  changing exactly twelve componentwise-median pixels
  ([`BASE-RENDERER-BEHAVIOR-MATRIX.md:54`](BASE-RENDERER-BEHAVIOR-MATRIX.md)).
  Note the coverage gate makes this partly downstream of D1's missing sidecar.
- **WM2000 reach.** UNKNOWN. Whether WM2000 latches any VI filter beyond D1
  has never been measured; the run aborts at D1 before reaching the divot
  check.
#### D10 — `G_AC_DITHER` alpha compare · **REACHES WM2000: same texrect path**

- **wgpu** `crates/fn64-render-wgpu/src/targets/texrect.rs:446`
  (`NoiseThresholdUnavailable`), raised at `texrect.rs:1381` and `:1812`.
- **reference** `crates/fn64-render-reference/src/raster/blend.rs:113` —
  `alpha * 256 > noise.byte() * 255`, cited to Programming Manual §15.5.4.
  Noise source at `crates/fn64-render-reference/src/raster/mod.rs:83-109`.
- **Disagreement.** The reference implements `G_AC_DITHER` and draws; wgpu
  refuses for want of an authoritative noise sequence.
  `crates/fn64-render-wgpu/src/alpha_compare.rs:129` already implements the
  identical arithmetic and lacks only the feed.
- **Which lane is right: UNKNOWN on the noise *byte*; REFERENCE on whether to
  draw at all.** Neither lane claims silicon authority for the sequence — the
  reference says so explicitly (`raster/mod.rs:88-90`, "deliberately not
  described as the silicon sequence") and wgpu quotes that accurately. The
  asymmetry worth naming: wgpu **already accepts** the "bounded endpoint"
  argument for `NOISE_DITHER_THRESHOLD` (`texrect.rs:1298`) and declines to
  accept it here, where the same bounding does not hold. That makes this the
  most defensible refusal in the table, and it is not scored as a wgpu defect.
- **WM2000 reach.** Same texrect path as D4.

#### D11 — YUV: refused at four layers, fully implemented by the reference · **REACHES WM2000: unmeasured**

- **wgpu refuses at four independent layers.**
  `crates/fn64-render-wgpu/src/tmem/texel.rs:509`
  (`YuvConversionDeferred`, decode);
  `crates/fn64-render-wgpu/src/tmem/wire.rs:631-634` ("YUV destination
  execution is deferred pending a public pairing contract", so no transfer plan
  is ever built); `crates/fn64-render-wgpu/src/tmem/types.rs:1116-1118`
  (`transfer_plan()` errors on `DeferredYuv`); and
  `crates/fn64-render-wgpu/src/tmem/execute/packet.rs:147-152`, where a YUV
  load **rejects the entire packet**, including the non-YUV loads sharing it.
- **reference implements the complete contract.**
  `crates/fn64-render-reference/src/gbi/state.rs:780-802` (`write_yuv_pair`:
  chroma U/V in the low 2 KiB, luma Y0/Y1 at `low + TMEM_HALF_BYTES`);
  `:884-897` (`TmemTexture::sample` YUV16, `high + (x & 1)` luma selection);
  `crates/fn64-render-reference/src/gbi/tmem.rs:202-229` (YUV `G_LOADTILE`,
  even-S/even-width validated); `:285-318` (YUV `G_LOADBLOCK` with DXT
  stepping); `:430-442` (direct texrect YUV16 decode, cited to the **SGI RDP
  Command Summary, Set Tile / Load Tile** notes). Tests at
  `crates/fn64-render-reference/src/gbi/tests/group4.rs:1015-1030`,
  `:118-132`, `:946-961`.
- **Disagreement.** wgpu's refusal rests on "a public pairing contract" not
  existing. It does exist — in the sibling lane, with a primary-source citation
  and byte-exact tests.
- **Which lane is right: REFERENCE.** Note also the blast radius: wgpu's packet
  layer fails *neighbouring* loads over this, which is a second defect
  independent of the YUV question.
- **WM2000 reach.** UNKNOWN. WM2000's known tiles are IA4 under `G_TT_RGBA16`
  and RGBA16; no YUV has been observed, in a window that has never reached
  gameplay.

#### D12 — Direct four-bit TMEM loads · **REACHES WM2000: plausible — IA4 tiles are measured**

- **wgpu** `crates/fn64-render-wgpu/src/tmem/execute/load_tile.rs:323`
  (`LoadTileExecutionError::DirectFourBit`, message at `:455`), and
  `crates/fn64-render-wgpu/src/tmem/wire.rs:776-778`, `:793-796` — *"direct
  four-bit TMEM loads are unsupported; load through a public 16-bit form."*
- **reference** `crates/fn64-render-reference/src/gbi/tmem.rs:127-130`
  (`source_texel` 4-bit via `packed_nibble`), `:154`
  (`assert_texture_source_range` 4-bit byte count), `:232-249` (the generic
  LoadTile loop passes `timg_siz` through unchanged, so 4-bit works), and
  `crates/fn64-render-reference/src/gbi/state.rs:757-759` (`write_texel`
  `G_IM_SIZ_4B` → `write_nibble` with per-nibble validity masking).
- **Disagreement.** wgpu has no 4-bit load path and directs callers to reshape
  the load. The reference does 4-bit source addressing and nibble-granular TMEM
  writes with exactly the partial-validity mask wgpu says it lacks.
- **Which lane is right: REFERENCE.** The asymmetry is *inside* wgpu: its
  **reader** already handles `Bits4` correctly
  (`crates/fn64-render-wgpu/src/tmem/read.rs:506-521`, `unpack_ci4_texel`).
  Only the load side refuses.
- **WM2000 reach.** Elevated. WM2000's measured tiles include **IA4 under
  `G_TT_RGBA16`** — a 4-bit format. Whether those tiles arrive by a direct
  4-bit load or a 16-bit-form load is not recorded by the census, so reach is
  plausible but unproven.

#### D13 — `NonCanonicalTlutEntry`: a write-side convention enforced as a read-side precondition · **REACHES WM2000: plausible — TLUT is on its path**

- **wgpu** `crates/fn64-render-wgpu/src/tmem/read.rs:578-611`
  (`read_canonical_tlut_entry`) requires **all eight bytes valid**
  (`:589-593`, `IncompleteTlutEntry`) **and all four 16-bit lanes equal**
  (`:601-606`, `NonCanonicalTlutEntry`).
- **reference** `crates/fn64-render-reference/src/gbi/state.rs:854-877`
  (`read_tlut`) reads **lane 0 only** — two bytes at
  `TMEM_HALF_BYTES + index * 8` and `+1` — and never inspects lanes 1-3.
- **Disagreement, stated precisely.** The reference *writer* does quadricate
  (`state.rs:841-852`, four banks), but its *reader* imposes no cross-lane
  agreement and no eight-byte validity requirement. wgpu promotes the write
  convention into a read precondition.
- **Which lane is right: REFERENCE on `NonCanonicalTlutEntry`.** The decisive
  point is an internal inconsistency: wgpu's own
  `crates/fn64-render-wgpu/src/tmem/execute/load_tlut.rs:811-822` deliberately
  supports arbitrary wrapping TLUT bases (base 511 across the bank), which
  produces exactly the unequal lanes `read.rs` then hard-refuses. **wgpu can
  write a state it will not read.** wgpu's own header concedes the refusal is
  not authority-backed (`read.rs:10-13`: "a conservative admitted subset;
  partial/unequal sample-lane behavior remains deferred to hardware
  measurement"). For `IncompleteTlutEntry` the two lanes differ only in
  strictness (reference traps on 2 invalid bytes, wgpu on 8) — same class,
  wgpu strictly broader; that half is **UNKNOWN**, not a defect.
- **WM2000 reach.** Elevated. WM2000 measurably runs tiles under
  `G_TT_RGBA16`, so the TLUT read path is live. Whether any of its TLUT state
  is non-canonical is unmeasured.

#### D14 — `EnabledCiSourceOutsideLowHalf`: a low-half constraint neither lane's sources impose · **REACHES WM2000: plausible — TLUT is on its path**

- **wgpu** `crates/fn64-render-wgpu/src/tmem/read.rs:493-500` — a CI read under
  an enabled TLUT whose first physical byte is at or above
  `TMEM_HIGH_HALF_BASE` is refused. Pinned at `read.rs:857-861` (a CI8 tile at
  `tmem: 256` under `Rgba16`).
- **reference** `crates/fn64-render-reference/src/gbi/state.rs:806-838`
  (`read_texel`) applies a low-half constraint **only** for `G_IM_SIZ_32B`
  (`:826-827`); the 4/8/16-bit arms address `base + x` across all 4 KiB.
  `state.rs:883-966` (`sample`) applies none on the TLUT-enabled path.
- **Disagreement.** wgpu restricts the *index source* to low-half TMEM. The
  reference's only low-half rules are for RGBA32 (`state.rs:766-767`,
  `:826-827`) and YUV (`:790-791`) — both genuine split-bank formats. A CI tile
  is not a split-bank format.
- **Which lane is right: REFERENCE on the divergence; UNKNOWN on hardware.**
  wgpu's header calls this "the canonical low-half source … frozen by
  M4.3.3b" — a self-citation, not a hardware citation. Neither lane cites a
  measurement of what silicon does with a high-half CI tile, so this row is
  scored reference-correct on *provenance* (one lane invents a constraint, the
  other does not) while the silicon answer stays open.
- **WM2000 reach.** Elevated, same reasoning as D13.
#### D15 — VI `osViFade` two-row interpolation · **REACHES WM2000: unmeasured**

- **wgpu** `crates/fn64-render-wgpu/src/vi_scanout.rs:92`
  (`ViScanoutRefusal::Fade`), raised at `vi_scanout.rs:322-324`.
- **reference** `crates/fn64-render-reference/src/vi.rs:49-70`.
- **Disagreement.** wgpu refuses `osViFade` by name. The reference interpolates
  between two framebuffer rows by the fade factor, and refuses only the genuine
  degenerate case ("osViFade requires at least two framebuffer rows").
- **Which lane is right: REFERENCE.** The interpolation is a documented libultra
  behavior with a published two-row rule; the reference implements it and names
  its one real precondition. wgpu names no precondition — it refuses the whole
  feature.
- **WM2000 reach.** UNKNOWN, same reason as D9.

---

### Tier C — unreachable today, or blocked behind another row

The VI rows here (D16–D18) are ordered behind D1 in
`admitted_filters` (`crates/fn64-render-wgpu/src/vi_scanout.rs:315-345`), so
the run aborts on silhouette AA before it ever evaluates them; D19–D21 are
gated behind an unresolved authority question or another row's refusal.

#### D16 — VI `osViRepeatLine` · **REACHES WM2000: unmeasured**

- **wgpu** `crates/fn64-render-wgpu/src/vi_scanout.rs:94`
  (`ViScanoutRefusal::RepeatLine`), raised at `vi_scanout.rs:325-327`.
- **reference** `crates/fn64-render-reference/src/vi.rs:71-72`.
- **Disagreement.** Identical shape to D15 — wgpu refuses, the reference
  implements the row-repeat.
- **Which lane is right: REFERENCE.** This is the smallest item in the table:
  the reference's implementation is a single branch.
- **WM2000 reach.** UNKNOWN, same reason as D9.
#### D17 — VI gamma dither · **REACHES WM2000: unmeasured**

- **wgpu** `crates/fn64-render-wgpu/src/vi_scanout.rs:90`
  (`ViScanoutRefusal::GammaDither`).
- **reference** `crates/fn64-render-reference/src/vi.rs:131-133` and `590-600`
  (`apply_gamma_dither`).
- **Disagreement.** wgpu's stated reason is that gamma dither "needs a
  retrace-seeded noise generator this module does not own." **That reason is
  stale.** Both halves are already public in the shared crate that wgpu
  *already depends on*: `fn64_render::vi_public_filters::`
  `gamma_dither_quantize_bounded_v1` (`crates/fn64-render/src/vi_public_filters.rs:56`)
  and `reference_noise_bit_v1` (`:63`). wgpu already imports a sibling from
  that exact module (`vi_scanout.rs:55` imports
  `restore_rgba16_component_bounded_v1`).
- **Which lane is right: REFERENCE, with a caveat.** The reference is explicit
  that its seed policy is "an explicit deterministic emulation policy," not a
  silicon claim (`vi.rs:1-7`, `585-589`). So the reference is not *hardware*
  correct here — but it is the workspace's declared policy, RT64's native lane
  ports the same mechanism (`vi-gamma-dither:v1`), and wgpu's refusal reason
  cites an unavailability that is factually not the case.
- **WM2000 reach.** UNKNOWN, same reason as D9.
- **RESOLVED (`1d0983e3`).** `ViScanoutRefusal::GammaDither` is removed —
  variant, reason arm, and admission-gate branch — and `vi_scanout.rs` now
  calls `gamma_dither_quantize_bounded_v1` with `reference_noise_bit_v1`, the
  same two shared functions the reference's `apply_gamma_dither` calls, over
  the same seed/pixel/channel keying. Applied last, after resampling, RGB only.
  The caveat in this row is preserved in the code: the quantizer half is the
  documented mechanism, the bit source is fn64's declared policy
  (`VI_PUBLIC_FILTER_POLICY_ID`), and `apply_gamma_dither`'s doc says so.
  `ViScanoutRefusal::Gamma` (D18) is untouched.

#### D18 — VI gamma curve · **UNKNOWN**

- **wgpu** `crates/fn64-render-wgpu/src/vi_scanout.rs:86`
  (`ViScanoutRefusal::Gamma`): "The silicon gamma ROM is not publicly
  specified; emitting a linear image while STATUS asks for gamma would be a
  wrong image, not a partial one."
- **reference** `crates/fn64-render-reference/src/vi.rs:128-130` and `569-579`
  (`apply_gamma`, `gamma_correct` = `(channel * 255).isqrt()`).
- **Disagreement.** wgpu refuses because the silicon curve is unpublished; the
  reference emits a deterministic integer square-root approximation.
- **Which lane is right: UNKNOWN, and both are honest.** The reference's own
  module header says the same thing wgpu's refusal says — "Public hardware
  descriptions specify the mechanisms below, but not the silicon gamma ROM ...
  The integer gamma curve ... [is an] explicit reproducibility polic[y], not
  [a] silicon-identical claim" (`vi.rs:3-7`). Neither lane claims hardware
  fidelity. This is a policy split (produce a documented approximation vs.
  refuse), not a correctness defect, and no evidence in this repo settles it.
  **Distinguish this row from D9/D15/D16**, where the mechanism *is* publicly
  specified and only wgpu declines to implement it.
- **WM2000 reach.** UNKNOWN, same reason as D9.
#### D19 — Bayer dither tile phase: RT64 vs reference · **UNKNOWN**

- **wgpu** `crates/fn64-render-wgpu/src/rgb_dither.rs:17-47` (module header,
  "Matrix cross-check against the existing reference oracle (frontier)") and
  the pinning test `rgb_dither.rs:420-450`
  (`bayer_matrix_disagrees_with_reference_oracle_at_documented_cells`). Consumed
  as a refusal at `crates/fn64-render-wgpu/src/targets/texrect.rs:460`
  (`OrderedDitherAuthorityUnsettled`).
- **reference** `crates/fn64-render-reference/src/raster/blend.rs:30`.
- **Disagreement, verified against upstream in this audit.** RT64's
  `DitherPatternBayer`
  (`/Users/jer/Code/no-mercy-recompiled/third_party/rt64/src/shaders/Formats.hlsli:9-14`)
  is `[[0,4,1,5],[4,0,5,1],[3,7,2,6],[7,3,6,2]]`; the reference's `BAYER` is
  `[[0,4,1,5],[6,2,7,3],[1,5,0,4],[7,3,6,2]]`. Rows 0 and 3 agree, rows 1 and 2
  differ. Both tiles contain every threshold `0..=7` exactly twice, so this is a
  phase/arrangement difference, not a malformed table.
  **`DitherPatternMagicSquare` is byte-identical between the two**
  (`Formats.hlsli:16-21` vs `blend.rs:29`), which is what makes the Bayer split
  a real anomaly rather than two unrelated transcriptions.
- **Which lane is right: UNKNOWN.** Checked in this audit and found to settle
  nothing: libultra `gbi.h`
  (`/Users/jer/Code/sm64-decomp/include/PR/gbi.h:661-671`) defines only the
  `G_CD_MAGICSQ`/`G_CD_BAYER` *selector bits* and publishes no table. No
  parallel-RDP checkout exists on this machine to consult as a third opinion.
  No hardware measurement exists. The wgpu lane's decision to refuse rather
  than pick a side is **correct given the evidence**; what is missing is the
  evidence, not the code.
- **WM2000 reach.** UNKNOWN — recorded as V4 in
  [`RT64-WM2000-REMAINING.md`](RT64-WM2000-REMAINING.md) with reach also
  unknown.
#### D20 — `fn64-render-wgpu` disagrees with *itself* on the Bayer table · **INTRA-CRATE, one-line fix candidate**

- **site A** `crates/fn64-render-wgpu/src/alpha_compare.rs:176` — `BAYER` is
  `[[0,4,1,5],[6,2,7,3],[1,5,0,4],[7,3,6,2]]`, the **reference** table, ported
  as "Literal port of `ordered_rgb_dither_threshold` (`blend.rs:28-38`)".
- **site B** `crates/fn64-render-wgpu/src/rgb_dither.rs` — the **RT64** table,
  ported from `Formats.hlsli`.
- **Disagreement.** One crate carries two different Bayer tiles for the same
  hardware quantity. `MagicSquare` is identical at both sites (both equal RT64
  and the reference, which agree), so the split is Bayer-only and is a direct
  consequence of the two modules choosing different upstreams.
- **Why this matters beyond bookkeeping.** libultra's alpha-dither `G_AD_PATTERN`
  is defined as *the selected RGB dither matrix*
  (`gbi.h:674-678`; `blend.rs:71-74` states the substitution rule). So the
  alpha-dither path and the RGB-dither path are required to read the **same**
  tile. Today they read different ones whenever Bayer is selected. At most one
  can be right, and they cannot both be right simultaneously.
- **Which lane is right: UNKNOWN which *table* is right (that is D19), but the
  *inconsistency* is unambiguously a defect.** Unlike D19 this needs no hardware
  evidence to act on: whichever table wins, both sites must use it.
- **WM2000 reach.** Gated behind D19's `OrderedDitherAuthorityUnsettled`
  refusal in the texrect path, so unreachable today. It becomes live the moment
  D19 is
  resolved — which is exactly when a silent wrong answer would ship.
- **RESOLVED (`b56454bc`).** `alpha_compare.rs`'s local `MAGIC_SQUARE`/`BAYER`
  constants are deleted; `ordered_dither_threshold` now calls
  `rgb_dither::ordered_tile_value`. **Table kept: `rgb_dither.rs`'s**, and the
  reason is `gbi.h:674-678` itself rather than a judgement about the
  arrangements — `rgb_dither.rs` *is* this crate's RGB dither module, so "the
  currently selected RGB dither matrix" is the thing it owns and alpha dither
  is downstream of it; keeping the other copy would have inverted the
  dependency libultra states. `the_alpha_dither_path_reads_this_modules_tables`
  pins the agreement over both selectors and all sixteen cells; restoring the
  duplicate makes it fail at Bayer `x=0 y=1` (6 vs 4). **D19 is untouched and
  still UNKNOWN** — this resolves the self-inconsistency only, and both module
  docs say so.

---

#### D21 — Disabled-TLUT CI4: wgpu implements *more* than the reference · **reverse direction**

- **wgpu** `crates/fn64-render-wgpu/src/tmem/texel.rs:377` aliases the
  normalized index to I8 on the TLUT-**disabled** CI4 path and returns a color.
- **reference** `crates/fn64-render-reference/src/gbi/state.rs:957-960` still
  routes to `tlut_color`, which **panics** on mode 0.
- **Disagreement.** This is the only row where wgpu is the broader lane. The
  two will disagree on output for disabled-TLUT CI4: wgpu returns a color,
  the reference aborts.
- **Which lane is right: UNKNOWN.** No source in this repo establishes the
  hardware behavior of a CI4 tile with the TLUT off. Recorded because it is a
  real behavioral split that this audit's search pattern would otherwise miss,
  and because a future convergence pass must decide it in one direction.
- **WM2000 reach.** UNKNOWN.

## 3. Refusals checked and found to be genuine agreement

These were audited and are **not** divergences — both lanes decline, for the
same stated reason. Listed so a later lane does not re-audit them.

- **`UnsupportedBlendShadeAlpha` / `UnsupportedColorInput{Shade}`**
  (`targets/texrect.rs:481`, `:376`). The reference agrees explicitly:
  "Rectangle commands carry no shade attributes. Validation rejects programs
  selecting SHADE, so zero is an inert and unreachable placeholder"
  (`crates/fn64-render-reference/src/raster/draw.rs:510-513`). Both lanes
  refuse.
- **`DepthModeDecision::UnsupportedInterpenetratingCoverageAdjustment`**
  (`depth_mode.rs:126`). The reference leaves the same case an explicit
  `unimplemented!` panic (`raster/coverage.rs:36,46-48`), and wgpu's module
  header says so. Both lanes refuse; wgpu's is the better-typed refusal.
- **`DitherRestorationNonRgba16`** (`vi_scanout.rs:80`). wgpu cites the
  reference's own matching refusal text and the reference does refuse it
  (`crates/fn64-render-reference/src/vi.rs:92`). Converged.
- **Three-nearest texture filter** (`shader_manifest.rs:1764-1815`). wgpu
  duplicates the reference's `filter_three_nearest_s10_5`
  (`gbi/types.rs:954-972`) literally, only because that function is
  `pub(super)` and not cross-crate reachable. Same arithmetic, no disagreement.
- **`UnsupportedIndexSize` at 32-bit under an enabled TLUT.** Both lanes refuse,
  for the same stated reason, near-verbatim: wgpu
  (`crates/fn64-render-wgpu/src/tmem/texel.rs:344-348`, `:374`) says the index
  byte "would have to be re-derived against the RGBA32 low/high bank split";
  the reference (`crates/fn64-render-reference/src/gbi/state.rs:934-941`, test
  `gbi/tests/group4.rs:1277-1295`) argues the same. Converged — but note that
  because the *reason* is shared, a future fix must land on both lanes.
- **`ReservedAlphaCompare`** (`targets/texrect.rs:476`). The reference panics on
  the same reserved encoding (`raster/blend.rs:8`, `:116`). wgpu's typed error
  is the better shape; the behavior agrees.
- **`InvalidTexelByte`** (`tmem/read.rs:309`). The reference has the identical
  uninitialized-TMEM trap (`gbi/state.rs:726-737`, matching panic text at
  `gbi/tests/group4.rs:943`). Converged. *This is the current all-Rust
  blocker's error type and it is **not** a lane divergence* — another lane owns
  the coverage bug behind it.
- **`Rgba32BaseOutsideLowHalf`** (`tmem/read.rs:310`). The reference asserts the
  same low-half rule for 32-bit (`gbi/state.rs:826-827`). Converged — and it is
  the contrast that makes D14's CI-tile constraint stand out as invented.
- **`PackedByteMustBeBits8`, `EntryMustBeBits16`, `IndexedDecodeIsSeparate`,
  `Ci4PaletteError`.** Internal type-narrowing preconditions on already-isolated
  values, not behavior refusals. No reference counterpart exists to disagree
  with.
- **`NonIntegralTexcoord` / `TexcoordOutOfRange`** (`targets/texrect.rs:402`,
  `:406`). Artifacts of wgpu's `f32`-to-S10.5 recovery in
  `try_from_viewport_and_texcoords`. The reference takes `rect.s`/`rect.t`
  already decoded and interpolates in `f32`
  (`raster/draw.rs:494-496`), so it never performs the recovery. **Different
  input contracts, no shared behavior to disagree about** — not scored.
- **`UnsetConstantRegister`** (`targets/texrect.rs:389`). No reference
  counterpart: the reference carries registers in `CombinerState` and never
  defaults them, so there is nothing to contradict.
- **VI five-bit channel expansion, `HeldLast` edge, `interpolate_u2_10`,
  `AxisSample` split, dither restoration** (`vi_scanout.rs:196-197`, `225-226`,
  `783-784`, `830-831`, `738-740`). All cite the reference and match it; the
  restoration filter literally calls the same shared entry point
  (`fn64_render::vi_public_filters::restore_rgba16_component_bounded_v1`) so
  the two cannot drift.

## 4. Resolved since the predecessor doc

- **TLUT over a non-CI tile** — the divergence that motivated this audit. Fixed
  at `4c412a96`, with 16-bit indexing through the high byte admitted and 32-bit
  still refused on both sides. The pinned-disagreement test in
  `crates/fn64-render-wgpu/src/tmem/texel.rs` is now a convergence test. This
  closes V5 in [`RT64-WM2000-REMAINING.md`](RT64-WM2000-REMAINING.md).

## 5. Named for a follow-up lane

Ranked by evidence quality against cost, not by size. **Nothing here was
changed by this audit** — each needs its own verification pass.

1. **D2 — widen `admitted_cycle_evaluates_combiner` to admit two-cycle.** The
   strongest one-line candidate in the table. `run_two_cycle` already exists
   and is public (`combiner.rs:1021`), and the refusal site itself records
   that "widening it to admit two-cycle left the entire suite green"
   (`targets/texrect.rs:1153-1155`). A follow-up lane still owes a test that
   *fails* before the widening — a green suite proves nothing was broken, not
   that anything was fixed.
2. **D20 — the intra-crate Bayer inconsistency.** The only row that needs no
   new hardware evidence to be worth acting on, because the two sites must
   agree regardless of which table wins.
3. **D3 — Fill-cycle texrect.** One match arm, an n64brew quote, and a
   witnessed WCW/nWo Revenge abort. The reference's route
   (`as_fill_cycle_rectangle` into the existing fill rasterizer) is already
   the shape to copy.
4. **D17 — correct the `GammaDither` refusal text at minimum.** It cites a
   generator it "does not own" that is public in
   `fn64-render::vi_public_filters` and already imported one line away
   (`vi_scanout.rs:55`). Even if the refusal stands, the stated reason is
   wrong.
5. **D16 — `osViRepeatLine`,** one branch in the reference.
6. **D1 — the highest-value row and the largest.** It needs the hidden-bits
   sidecar, which also unblocks D5, D8 and D9. Not a one-liner; it is the item
   that actually gets WM2000 past its first frame.

**Not recommended as quick fixes**, despite appearing in the reference-correct
column: D4 (twelve combiner inputs, each needing its own evidence), D11 (YUV, a
four-layer contract), and D13/D14 (both require deciding what wgpu's loader
should be allowed to produce before changing what its reader accepts).

## 6. What this audit could not establish

- **Which Bayer tile is the RDP's** (D19). Not settled by `gbi.h`, not settled by
  RT64 (RT64 *is* one of the two disputants), and no parallel-RDP checkout
  exists on this machine. Prior lanes' notes cite parallel-RDP second-hand only;
  that is recorded here as second-hand and was not used as evidence.
- **Whether WM2000 latches any VI filter beyond D1.** The run aborts at the
  first present, so D9 and D15–D18 have never been reached. The census decodes
  opcodes,
  not `G_RDPSETOTHERMODE` payload bits or VI STATUS, so it cannot answer this
  either.
- **Whether WM2000 selects a coverage-consuming blend mode** (D8). Same census
  limitation.
- **The `RT64-WM2000-CENSUS.md` window caveat applies to every "unmeasured" row
  above.** Those counts describe a 219-decode-entry window since superseded
  twice (to 2,219 then 5,792 entries). An absence there means "not seen in
  boot/logo/attract" and never "does not occur"; that misreading already caused
  one wrong refusal.
- **What silicon does with a high-half CI tile** (D14). Neither lane cites a
  measurement. The row is scored on provenance — one lane invents a constraint,
  the other does not — and the hardware answer stays open.
- **What silicon does with a disabled-TLUT CI4 tile** (D21). The two lanes
  actively disagree in output (wgpu returns a color, the reference panics) and
  nothing here settles it.
- **Whether unequal TLUT sample lanes are readable** (D13). wgpu's own header
  concedes the question is "deferred to hardware measurement"
  (`tmem/read.rs:10-13`).
- **The RDP's per-pixel random sequence** (D10, and D19's noise arm). Both
  lanes state plainly that their generators are policies, not silicon. This is
  a permanent caveat, not a gap to close.
- **No hardware comparison has ever been made** for any row in this table.
