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

## 1. Headline

**Nine pinned divergences. Six are wgpu-side defects — the reference lane
already implements the behavior — and one of those six sits on WM2000's very
first frame.**

| Verdict | Count |
|---|---|
| **Reference-correct** (wgpu refuses, reference implements) | 6 |
| **wgpu-correct** (wgpu refuses correctly, or reference over-claims) | 0 |
| **UNKNOWN** (no evidence in this repo settles it) | 3 |

The single largest structural cause is not nine independent bugs: **five of the
six reference-correct rows trace to one missing datum.** `fn64-render-reference`
keeps a 195-line per-pixel coverage sidecar
(`crates/fn64-render-reference/src/backend/hidden_bits.rs`, `RdramHiddenBits`)
that `fn64-render-wgpu` does not maintain. Every wgpu refusal that names
"coverage this backend does not track" is downstream of that one absence.

---

## 2. The table, ranked by whether WM2000's measured path reaches it

### D1 — VI silhouette antialiasing (AA modes 0/1) · **REACHES WM2000: FIRST FRAME**

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

### D2 — Blender `B = FramebufferAlpha` / destination coverage · **REACHES WM2000: unmeasured, same root cause as D1**

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

### D3 — VI divot filter · **REACHES WM2000: unmeasured**

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
- **WM2000 reach.** UNKNOWN. Whether WM2000 latches VI filters beyond D1 has
  never been measured; the run aborts at D1 before reaching the divot check.

### D4 — VI `osViFade` two-row interpolation · **REACHES WM2000: unmeasured**

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
- **WM2000 reach.** UNKNOWN, same reason as D3.

### D5 — VI `osViRepeatLine` · **REACHES WM2000: unmeasured**

- **wgpu** `crates/fn64-render-wgpu/src/vi_scanout.rs:94`
  (`ViScanoutRefusal::RepeatLine`), raised at `vi_scanout.rs:325-327`.
- **reference** `crates/fn64-render-reference/src/vi.rs:71-72`.
- **Disagreement.** Identical shape to D4 — wgpu refuses, the reference
  implements the row-repeat.
- **Which lane is right: REFERENCE.** This is the smallest item in the table:
  the reference's implementation is a single branch.
- **WM2000 reach.** UNKNOWN, same reason as D3.

### D6 — VI gamma dither · **REACHES WM2000: unmeasured**

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
- **WM2000 reach.** UNKNOWN, same reason as D3.

### D7 — VI gamma curve · **UNKNOWN**

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
  **Distinguish this row from D3/D4/D5**, where the mechanism *is* publicly
  specified and only wgpu declines to implement it.
- **WM2000 reach.** UNKNOWN, same reason as D3.

### D8 — Bayer dither tile phase: RT64 vs reference · **UNKNOWN**

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

### D9 — `fn64-render-wgpu` disagrees with *itself* on the Bayer table · **INTRA-CRATE, one-line fix candidate**

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
- **Which lane is right: UNKNOWN which *table* is right (that is D8), but the
  *inconsistency* is unambiguously a defect.** Unlike D8 this needs no hardware
  evidence to act on: whichever table wins, both sites must use it.
- **WM2000 reach.** Gated behind D8's `OrderedDitherAuthorityUnsettled` refusal
  in the texrect path, so unreachable today. It becomes live the moment D8 is
  resolved — which is exactly when a silent wrong answer would ship.

---

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

Ranked by evidence quality, not size. **Nothing here was changed by this
audit.**

1. **D9** — the intra-crate Bayer inconsistency. The only row that needs no new
   evidence to be worth acting on, because the two sites must agree regardless
   of which table wins.
2. **D6** — wgpu's `GammaDither` refusal cites a generator it "does not own"
   that is public in `fn64-render::vi_public_filters` and already imported one
   line away. At minimum the refusal text is wrong and should be corrected even
   if the refusal stands.
3. **D5** — `osViRepeatLine`, one branch in the reference.
4. **D1** — the highest-value row and the largest: it needs the hidden-bits
   sidecar, which also unblocks D2 and D3.

## 6. What this audit could not establish

- **Which Bayer tile is the RDP's** (D8). Not settled by `gbi.h`, not settled by
  RT64 (RT64 *is* one of the two disputants), and no parallel-RDP checkout
  exists on this machine. Prior lanes' notes cite parallel-RDP second-hand only;
  that is recorded here as second-hand and was not used as evidence.
- **Whether WM2000 latches any VI filter beyond D1.** The run aborts at the
  first present, so D3–D7 have never been reached. The census decodes opcodes,
  not `G_RDPSETOTHERMODE` payload bits or VI STATUS, so it cannot answer this
  either.
- **Whether WM2000 selects a coverage-consuming blend mode** (D2). Same census
  limitation.
- **The `RT64-WM2000-CENSUS.md` window caveat applies to every "unmeasured" row
  above.** Those counts describe a 219-decode-entry window since superseded
  twice (to 2,219 then 5,792 entries). An absence there means "not seen in
  boot/logo/attract" and never "does not occur"; that misreading already caused
  one wrong refusal.
- **No hardware comparison has ever been made** for any row in this table.
