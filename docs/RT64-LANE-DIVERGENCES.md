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
  `MixedTexrectAndRawTrianglePacket` (D24, below).

### D24 — raw-DPC execution refused a texrect composed with a raw triangle · **REACHED WM2000: FIFTH ABORT**

- **wgpu** `crates/fn64-render-wgpu/src/production.rs`, `stage_and_report`,
  surfacing as `WgpuRawDpcExecutionError::MixedTexrectAndRawTrianglePacket`
  and aborting the all-Rust stack at
  `crates/fn64-abi/src/task_dispatch/rsp_commit.rs:1202`.
- **The refused packet, measured — not inferred.** Instrumented at the
  refusal site and run on the real ROM through the all-Rust lane
  (`FN64_RECOMP=rs`, `FN64_RENDER=wgpu`): **6 texrects, 9 TMEM loads, 1 raw
  triangle, 0 fills, 0 syncs** — 13 admitted triangles in all (each texrect
  admits as two). The raw triangle is **strictly last**, at wire command 91,
  after every texrect (commands 2, 10, 18, 26, 34, 42) and every load (7,
  15, 23, 31, 39, 54, 58, 81, 88). The texrects are a HUD strip: viewports
  `336,144–399,192` then five tiles across `80..399` at `y 192–239`. Every
  one declared a real 47- or 48-access `ColorFramebuffer` write run.
  **There is no interleaving of the two sources at all.**
- **Which lane was right: NEITHER — the refusal was wrong on its own
  terms.** Its message said the pair "have no defined ordering". Both
  clauses of its reasoning are individually true — a texrect declares
  journal writes, a raw triangle declares none — but the conclusion does not
  follow, because the composition it names is never attempted:
  - A texrect's pixels reach the guest through `stage_color_commands` →
    `ColorTargetRegistry` → `fn64-abi`'s `copy_committed_guest_writes`.
  - A raw triangle's raster reaches `triangle_draw_output`, which
    `last_triangle_draw`'s own doc calls "never an accumulated history,
    never a persistent framebuffer" and which `present` refuses to scan out
    by name: "one submission's readback, not a VI-sampled framebuffer".
    **Nothing copies it into guest RDRAM.**

  So the packet has exactly one guest-visible destination and exactly one
  source writing to it, and that order was already derived — from the
  decoder's own `command_index` in `stage_color_commands`, cross-checked by
  `merged_fill_and_tmem_writes`' independent re-derivation from the journal.
- **Admitting adds nothing for the journal to order.** A `RawTriangle`
  pushes no `ResourceAccess` (the decoder's `0x08..=0x0f` arm decodes and
  pushes the command; unlike `FILL_RECTANGLE`/`TEXRECT` it calls no
  planner), so it contributes neither a declared write nor a staged
  `CompletedWrite`. `merged_fill_and_tmem_writes`' two-sided exactness check
  sees the identical pair of lists it would see with the triangle absent.
- **What the refusal cost.** It dropped six real guest-visible rectangles in
  order to withhold one triangle that was never going to be visible — not in
  this packet and not in a triangle-only one, where
  `StagedOutcome::NoPhysicalSuccessor`'s own doc already records that "a raw
  triangle rasters into a GPU attachment and declares no journal write" and
  the packet is admitted anyway. The missing RDRAM writeback for the GPU
  raster path is a separate, pre-existing gap that refusing could not close.
- **Status: FIXED.** The variant is removed. `MixedFillAndTrianglePacket` is
  **kept and unchanged** — that pair was never measured in WM2000's stream
  and its own routing question is different. Per-triangle TMEM needed no
  widening: `project_pending_tmem_per_triangle` selects with `prefix_before`,
  a fact about stream position and not triangle source, so the raw triangle
  at command 91 correctly samples the prefix sealed by the load at command
  88.
- **Mutants: three of three killed.** (1) restoring the refusal fails the new
  admission test; (2) deleting the **kept** `MixedFillAndTrianglePacket` arm
  fails three existing tests, so that arm is genuinely covered; (3)
  over-widening the kept arm to cover texrects fails the new admission test.
  The removed arm had **zero** tests before this card — the untested-kept-arm
  hazard, found by looking.
- **The run now advances to `TmemSampleFailed { status: 2 }`
  (`TMEM_SAMPLE_STATUS_INVALID_BYTE`), triangle #0 in plan order, tile format
  code 3 (`IntensityAlpha`), pixel-size code 0 (`Bits4`), TLUT-mode code 2.**
  That is the GPU raster half of the very packet this card admitted: the
  fragment shader addressed a TMEM byte the projection reports invalid, for
  an IA4-under-TLUT tile. Note the asymmetry — the CPU texel reader composed
  the same texrects successfully; only the WGSL sampler failed.

### D25 — the WGSL sampler hardcoded the tile's first-row parity · **REACHED WM2000: SIXTH ABORT**

- **wgpu** `crates/fn64-render-wgpu/src/shaders/tmem_sample.wgsl:71`,
  `const TMEM_FIRST_ROW_PARITY_ODD: bool = false;`, consumed by
  `tmem_rgba16_byte_address` (`:257-268`) and surfacing as
  `WgpuRawDpcExecutionError::TmemSampleFailed { status: 2 }`
  (`TMEM_SAMPLE_STATUS_INVALID_BYTE`) at
  `crates/fn64-abi/src/task_dispatch/rsp_commit.rs:1202`.
- **This is `aa6f644e`'s reader/writer parity inversion, one layer over.**
  The CPU reader takes `TmemFirstRowParity` as explicit caller input
  (`tmem/read.rs:65-72`: "This is explicit caller input. The reader never
  infers it"), and `aa6f644e` fixed `targets/texrect.rs:1237` to derive it
  from the tile's own T origin so the reader matches the *writer's* rule
  (`tmem/types.rs`'s `project_tmem_transfer_word`, `Tile` arm:
  `odd_row_exchange = (bounds.low_t().integer() + row) & 1`). The WGSL
  sampler was never given the same derivation: it froze `Even`.
- **The measured delta, hand-derived from the wire fields, not captured.**
  WM2000's strip tile is `tmem = 0`, `line_words = 5`, `low_t_raw = 188`, so
  `low_t.integer() == 47`, **odd**. For the failing texel (tile column 64,
  tile row 1, 4-bit) the linear address is
  `0*8 + 1*5*8 + 64/2 == 0x048`. The writer's own rule exchanges that row
  (`(47 + 1) & 1 == 0` → no exchange on row 1, exchange on row 0), so
  `0x048` is a byte the `LoadTile` at cmd 39 really wrote. The frozen `Even`
  parity computes `first_is_odd(false) ^ (row&1 == 1) == true` and XOR4s it
  to **`0x04c`** — a byte inside the 6-byte per-row tail gap that the load
  never wrote. **Shader read `0x04c`; CPU reader reads `0x048`.** Both
  addresses are already pinned by
  `tmem::read::tests::wm2000_texrect_pixel_sixty_three_reproduces_the_production_invalid_byte`,
  which asserts `valid_byte(0x048).is_some()` and
  `valid_byte(0x04c).is_none()` — i.e. the delta is a pure XOR-4 parity
  inversion, not a stride, a base offset, or a projection gap. The
  projection is innocent: `TileBindingParams` already uploads `low_t`
  (`gpu_projection.rs:212`), so the shader had the information and did not
  read it.
- **Why the existing GPU-vs-CPU pinning test could not catch it.**
  `48cde862` added
  `required_host_enabled_tlut_over_an_ia4_tile_samples_and_matches_the_cpu_reader`,
  which does compare the shader against `crate::read_texel` over the same
  bytes — but it passes `TmemFirstRowParity::Even` to the CPU side, and its
  `tlut_fixture` tile has `low_t == 0` (**even**) with only two rows. Both
  lanes therefore agreed on parity vacuously. **The fixture and the real
  tile differ in exactly the axis that broke**, which is itself the finding:
  a differential test pinned against a hardcoded constant only pins the
  constant.
- **The general hazard, stated once for every differential in this crate.**
  A CPU/GPU differential proves the two lanes agree only over the inputs the
  fixture actually varies. Where the shader hardcodes a value and the test
  hands the CPU oracle *that same value*, the comparison is a tautology: it
  cannot fail no matter what either lane does with it. Both of this crate's
  enabled-TLUT differentials had that shape for first-row parity. **When
  adding or reviewing one, list the inputs the shader treats as constants
  and check the fixture varies at least one of them off the constant** —
  otherwise the test pins the constant rather than the behaviour.
- **Status: FIXED.** No new wire field was needed: `TileBindingParams`
  already uploads `low_t`, and `TileCoordinate::integer()` is `raw >> 2`,
  so `tmem_first_row_parity_odd` reads `(low_t >> 2) & 1` — the same one
  rule `targets/texrect.rs:1237` applies, over the same word, rather than
  two constants that can drift. No refusal was weakened: `INVALID_BYTE`
  still fires for a genuinely unwritten byte; the shader now asks about the
  right byte.
- **Composes with `D-LOWHALF` (`852d20e9`), which landed alongside it in the
  same file.** That guard refuses an enabled-TLUT CI source at or above
  `0x0800`, and its own doc requires the check to run on the FULLY addressed
  byte, "post twelve-bit wrap, post odd-row XOR4 exchange". It was therefore
  testing a wrong address for an odd-origin tile too — its call site is the
  fifth `tmem_rgba16_byte_address` caller and now takes the tile like the
  other four. The guard itself is unchanged and is not this card's to
  revisit; D14 holds its open ruling. The new GPU test asserts every
  fragment is `STATUS_OK` rather than merely "not `INVALID_BYTE`", so a
  wrongly-tripped low-half guard fails it as well.
- **`TRIANGLE_PIPELINE_FRAGMENT_*` digests were recomputed over the MERGED
  shader** (100,427 → 102,101 bytes). Both this fix and `852d20e9` refroze
  them after editing the same file, so *neither* lane's frozen value was
  right once both edits were present — resolving that conflict by keeping
  either side leaves a digest inconsistent with the shader, which surfaces
  as `a_composed_packet_reports_writes_in_the_streams_own_journal_order` and
  `a_failed_triangle_draw_leaves_no_redeemable_fill_token` failing. The
  recomputation, not either side, is the resolution.

### D26 — the raw-DPC triangle path pairs a tile descriptor with the wrong TMEM prefix · **REACHED WM2000: SEVENTH ABORT**

- **wgpu** `crates/fn64-render-wgpu/src/production.rs`, `stage_and_report`'s
  triangle batch, surfacing (still) as
  `WgpuRawDpcExecutionError::TmemSampleFailed { status: 2 }` at
  `crates/fn64-abi/src/task_dispatch/rsp_commit.rs:1202`.
- **The abort survives D25's parity fix, and the packet is NOT the one D24
  measured.** Instrumented with a temporary `eprintln!` at the refusal site
  (`stage_and_report`, `production.rs`) and at
  `project_pending_tmem_per_triangle`'s call site, run on the real ROM
  through the all-Rust lane (`FN64_RECOMP=rs`, `FN64_RENDER=wgpu`). That
  instrumentation was removed before commit, so every figure below is a
  measurement this doc records, not one the tree still prints. The failing
  batch is **five triangles, all texrect-sourced, no raw triangle at all**,
  and **every one of the five has `low_t == 0` — an EVEN T origin**. The
  odd-origin `low_t.integer() == 47` tile D25 names is a *texrect-lane* fact
  (`tmem::read::tests`'s fixtures); the GPU batch that aborts carries
  different tiles. D25's fix is correct and necessary but is not this
  abort's cause.
- **The measured divergence is the row stride, and it is a PAIRING defect,
  not an addressing one.** Per-fixture, replaying the shader's own
  addressing over every texel the tile can address:

  | tri | declared `line_words` | projection valid bytes | texels hitting an invalid byte | `line_words` values that fit the projection |
  |-----|----|------|-----------|-----------|
  | 0 | **4** | 1828 | **473 of 3185** | **[5]** |
  | 1 | 5 | 1828 | 0 of 3234 | [5] |
  | 2 | 5 | 1828 | 0 of 3234 | [5] |
  | 3 | 4 | 2055 | 0 of 3185 | [1, 2, 3, 4] |
  | 4 | 4 | 2055 | 0 of 3185 | [1, 2, 3, 4] |

  Triangle 0 declares a **4-word (32-byte)** row stride while the TMEM
  prefix it was handed was written with a **5-word (40-byte)** one. The
  projection's valid set is a clean repeating 80-byte (two-row) figure —
  `0x0000..=0x0021` (34 bytes), gap, `0x0028..=0x0047` (32), gap,
  `0x004c..=0x004d` (2), gap — which is exactly a 5-word-per-row `LoadTile`
  writing 34 defined bytes per 40-byte row with the XOR4 exchange on
  alternating rows. At stride 4 the shader reads row 1 at
  `1 * 4 * 8 == 0x20`, XOR4s to `0x24`, and lands in the `0x22..=0x27` gap.
  **Shader read `0x24`; the byte the same texel has under the stride its own
  data was written with is `0x28`.**
- **Tris 3 and 4 are the control.** They declare the same `line_words = 4`
  and sample cleanly — because their projection (2055 valid bytes) is a
  *different, later* prefix that genuinely fits stride 4. So `line_words = 4`
  is not wrong in itself, and the descriptor is not corrupt. Triangle 0's
  descriptor and triangle 0's TMEM prefix simply come from different points
  in the stream.
- **Which triangle takes which image, measured.** `triangle_commands` for
  the failing packet is `[0, 7, 8, 15, 16]` and the packet's two completed
  loads capture prefixes at command indices `[4, 12]`. Applying
  `prefix_before`:

  | tri | command | image `prefix_before` selects | valid bytes | stride the image really has | tile's declared `line_words` |
  |-----|---------|-------------------------------|-------------|------------------------------|------|
  | 0 | **0** | **committed** (no load precedes it) | 1828 | **5** | **4** |
  | 1 | 7 | prefix@4 | 1828 | 5 | 5 |
  | 2 | 8 | prefix@4 | 1828 | 5 | 5 |
  | 3 | 15 | prefix@12 | 2055 | 4 | 4 |
  | 4 | 16 | prefix@12 | 2055 | 4 | 4 |

  Four of the five agree. **Triangle 0 is the packet's very first command**,
  so no load in this packet precedes it and `prefix_before` returns `None` —
  the durable committed arm. That arm is not a fallback; the absence of a
  preceding load genuinely is the stream fact that makes committed correct.
  But the committed state here still holds the *previous* packet's
  5-word-stride image, while triangle 0's own tile descriptor declares
  `line_words = 4`.
- **The committed slot is NOT stale — measured, not assumed.** The two
  strides alternate *within* packets, and every other pairing is right, so
  the selection rule is working. Instrumenting each prefix image and the
  committed slot side by side:

  ```
  packet N-1: prefix#0 @cmd  6: valid=2055   (stride 4)
              prefix#1 @cmd 14: valid=1828   (stride 5)
              prefix#2 @cmd 22: valid=1828   (stride 5)
              committed (on entry): valid=2055
              triangle_commands = [9, 10, 17, 18, 25]
              tile line_words    = [4, 4, 5, 5, 5]
  packet N:   prefix#0 @cmd  4: valid=1828   (stride 5)
              prefix#1 @cmd 12: valid=2055   (stride 4)
              committed (on entry): valid=1828
              triangle_commands = [0, 7, 8, 15, 16]
              tile line_words    = [4, 5, 5, 4, 4]
  ```

  In packet N-1 the tri at cmd 9/10 declares stride 4 and takes prefix@6
  (2055, stride 4) — correct. The tris at 17/18/25 declare 5 and take
  prefix@14/@22 (1828, stride 5) — correct. Packet N's committed slot is
  `1828`, exactly packet N-1's last load. **Publication is working and the
  slot is current.** Five of the six pairings across the two packets agree.
- **The one that does not is packet N's triangle 0, and it is a
  cross-packet fact, not an intra-packet one.** It sits at command index
  **0** — the very first command in its packet — and declares
  `line_words = 4`, so the stride-4 image it wants is the one packet N-1
  loaded at its command 6 and then *overwrote twice*. `prefix_before`
  cannot reach it: it is not in packet N, and it is no longer what
  committed holds. The rule is correct within a packet and blind across
  the boundary.
- **Status: OPEN, and the next lane must separate two readings this lane
  did not.**
  1. **The packet boundary is drawn in the wrong place** — these two
     "packets" are one guest drawing sequence split by the task-dispatch
     seam, and triangle 0 belongs with packet N-1's stride-4 load. If so
     the fix is upstream of `fn64-render-wgpu` entirely.
  2. **The guest genuinely re-draws a sprite whose TMEM was overwritten**,
     and hardware samples the gap bytes as undefined-but-not-fatal while
     this lane refuses them.
  Reading 2 would put the `INVALID_BYTE` refusal itself in scope — but
  **not to be weakened blind**: `3a1a6a73` measured that exact status
  catching a genuinely missed load, and D25's mutation run (below)
  confirmed the arm is live and covered by
  `required_host_a_non_canonical_tlut_entry_is_refused_not_guessed`.
  Nothing was changed for D26.

---

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

**Since measurement, one verdict has been overturned by later work rather
than by re-argument.** D7 was scored a wgpu defect because the alpha-dither
stage read a table that agreed with the reference while the disputed one
lived in the RGB module. `51b4e184` deleted that duplicate -- correctly, since
libultra makes the two paths read one table by definition -- so the alpha
stage is now downstream of the disputed tile and the refusal is right.
**Reclassify D7 from wgpu-side defect to blocked on D19**, which makes the
standing counts 14 defects / 0 wgpu-correct / 6 UNKNOWN / 1 blocked-on-UNKNOWN.

The general shape is worth carrying forward: a row whose evidence is *"two
sites in one crate disagree"* can be discharged by making them agree, which
changes the verdict without anyone touching the refusal it was scoring.

Three structural causes account for eleven of the fifteen. This matters for
sequencing: they are not eleven independent fixes.

1. **One missing datum.** `fn64-render-reference` keeps a 195-line per-pixel
   coverage sidecar
   (`crates/fn64-render-reference/src/backend/hidden_bits.rs`,
   `RdramHiddenBits`) that `fn64-render-wgpu` does not maintain. Every wgpu
   refusal naming "coverage this backend does not track" is downstream of that
   one absence: **D1, D5, D8, D9.**

   **Amended after working the rows.** Two corrections, both in the
   direction of the cause being *narrower but deeper* than stated. (a) It is
   not one datum but two: the sidecar holds the low two bits, and the
   reference's rasterizer supplies the third through the visible LSB, which
   on the wgpu lane holds `alpha >> 7` instead -- so wgpu recovers none of
   the three bits, not one of three, and a sidecar alone would widen a field
   nothing writes. D1's and D8's outcome blocks carry the sizing. (b) **D5
   does not belong on this list except in one of its four cases.** Its
   `wraps` term is a conjunction gated on `IM_RD`, so three of the four
   `FORCE_BL`-clear cases settle with no coverage count at all; only the
   fourth is downstream of the sidecar. Landed as `7ebe0647`.
2. **Wiring gaps described as capability gaps.** `fn64-render-wgpu`'s own
   `combiner.rs`, `blend.rs`, `coverage.rs`, and `alpha_compare.rs` already
   implement behaviors that `targets/texrect.rs` refuses as unimplementable.
   Four refusal doc comments are factually contradicted by sibling modules in
   the same crate: **D2, D4, D5, D7.**

   **Amended.** The pattern is real and was right about D2, D4 and D5, but
   "a sibling module implements it" is necessary and not sufficient, and two
   of the four rows needed splitting on exactly that. `crate::combiner`
   implementing a selector means it can *read* a `CombinerInputs` field, not
   that the executor can *fill* it -- so D4 is five wiring gaps and seven
   genuine ones (`e50789de`). And D7's sibling module stopped contradicting
   the refusal when `51b4e184` deduplicated its table. When applying cause 2,
   check what feeds the sibling's inputs, not only that the sibling exists.
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

- **SIZED, NOT LANDED (`4d7a45ac`). The refusal is kept, and the sidecar is
  necessary but not sufficient.** The verdict above stands -- the reference
  is right and wgpu is the only one of three lanes refusing -- but this
  row's attribution to *one* missing datum is incomplete, and the missing
  half changes the estimate.
  1. **The sidecar is genuinely required.** `SourcePlane::coverage` can
     express only 8 or 1, never 2..=7, and silhouette AA consumes the
     magnitude as the blend weight `coverage/8` (`vi.rs:281-291`). A pixel
     the RDP wrote at coverage 4 would blend 1/8 foreground against 7/8
     estimated background. So cause 2 does **not** apply here: this is not
     a wiring gap.
  2. **A sidecar alone would not be enough, because this lane never
     produces coverage to put in one.** The reference's sidecar is
     populated by its own rasterizer -- `write_rgba5551_framebuffer` splits
     `Coverage::stored()`, bit 2 into the visible halfword and bits 0..=1
     into the sidecar (`backend/framebuffer_io.rs:143-190`), so on that
     lane the visible LSB really is the coverage MSB. On the wgpu lane that
     bit is **alpha**: `targets::pack_device_pixels`
     (`targets/oracle.rs:128-131`) and the private `write_pixel`s in
     `targets/fill.rs` and `targets/texrect.rs` all emit `alpha >> 7`, and
     `crate::coverage::Coverage::stored()` has zero production callers.
     wgpu's committed RDRAM carries no coverage information in any bit.

  The real shape is two pieces, in order: a coverage stage that computes
  and *retains* a per-pixel count through the draw path (the executors
  derive one and discard it today -- `targets/texrect.rs`'s `coverage_for`
  says so itself), then the 195-line sidecar for the two bits RGBA16 has no
  room for. Building only the sidecar would produce a filter weighted by a
  count that is still always 8 or 1. Pinned by
  `this_lanes_rgba16_low_bit_is_alpha_not_a_coverage_msb`; the kept arm's
  mutant is killed by three tests.
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

- **PARTLY RESOLVED (`e50789de`): five of the twelve landed, seven kept.**
  The premise is right and the conclusion needed splitting. `crate::combiner`
  implementing a selector means it can *read* a `CombinerInputs` field, not
  that the executor can *fill* it.

  **Admitted** -- each resolves to a component of a value the executor
  already sources from a real wire register, so evaluating it invents
  nothing: `ColorInput::Texel0Alpha` (`texel0[3]`, the sampled texel's own
  alpha), `ColorInput::PrimitiveAlpha` (`prim_color[3]`),
  `ColorInput::EnvAlpha` (`env_color[3]`), and `PrimLodFrac` on both the
  color and alpha sides (`PrimColor::lod().lod_frac_normalized()`, wired by
  `combiner_inputs_from_fragment_registers`). The reference admits all of
  these for rectangles.

  **Kept, and this row should say so:** `LodFraction`, `Noise`, `K4`, `K5`,
  `KeyCenter` and `KeyScale` read `TexrectShading::base_inputs` fields left
  at **zero** -- there is no `SetConvert`/`SetKey` plumbing, no LOD stage,
  and no noise authority. Admitting them would combine against an invented
  zero, the failure the `Shade` refusal exists to prevent.
  `Texel1`/`Texel1Alpha` stay refused because a rectangle binds one tile,
  which is the reference's own reason (`validate.rs:479-483`).

  The widening exposed a second, load-bearing half: `validate_combiner_
  program`'s `reads_env`/`reads_prim` matched only the plain
  `Environment`/`Primitive` variants, so admitting `EnvAlpha` /
  `PrimitiveAlpha` / `PrimLodFrac` without widening that detection would
  have let a program reading them with no `SetEnvColor`/`SetPrimColor`
  staged fall through to `base_inputs`' `unwrap_or(Color4::from_wire(0))`
  and combine against black. Both matches are widened.

  Note for future rows: the pre-existing exhaustive sweep derived its
  expectation from `ADMITTED_COLOR_INPUTS` itself, so it could not fail
  when that constant changed. The new test
  `register_backed_selectors_are_admitted_and_invented_ones_are_not` is
  hand-derived instead. Four mutants killed, including the over-widen that
  admits `K4`.

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

- **RESOLVED (`7ebe0647`), and the refusal narrows rather than disappears.**
  The row is right that the reference is the authority, but the refusal was
  over-broad by exactly one conjunct rather than wholly wrong.
  `wraps = image_read_enabled && sum > 8` is a **conjunction whose first
  term is `image_read`**, so a clear `IM_RD` pins `wraps` to `false`
  without the sum being formed at all, and `blend_enabled` collapses to
  `antialias_enabled()`. No sidecar, no coverage stage, no D1 dependency:
  `require_blendable_mode` now requires all three of `!FORCE_BL`, `AA_EN`
  and `IM_RD` before refusing. The genuinely underivable case
  (`FORCE_BL` clear, `AA_EN` set, `IM_RD` set) still refuses by name, and
  that arm's mutant is killed by two tests.

  This row's placement under structural cause 1 (the missing sidecar) is
  therefore only two-thirds right: the sidecar bounds the *last* case, not
  the whole refusal.

  The widening exposed a second half the row does not mention:
  `blend_texrect_fragment` hardcoded `blend_enabled = force_blend()`,
  justified by an admitted set that no longer held. On the newly admitted
  mode that expression is `false` where the RDP's is `true`, which would
  have bypassed the blender silently. It is now the reference's disjunction
  with `wraps` pinned `false` -- provably exact on every admitted mode.
  Four mutants killed. WM2000's own mode (`0x005041c8`) sets all three
  bits, so its behavior is unchanged.

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

- **VERDICT SUPERSEDED (`9063f83c`): the refusal is right after all, and
  this row's premise expired between the audit and now.** The argument
  above is sound *as of `4371d57a`* -- `alpha_compare.rs:175-176` really did
  hold a `BAYER` byte-identical to the reference's, so the stage being
  refused really did agree with the reference cell-for-cell.

  **`51b4e184` deleted that duplicate.** libultra defines `G_AD_PATTERN`'s
  threshold as *the currently selected RGB dither matrix*
  (`gbi.h:674-678`), so one hardware quantity having two tables in one
  crate was itself the defect -- whichever table is right, at most one of
  the two sites could have been. `alpha_compare.rs` now reads
  `crate::rgb_dither::ordered_tile_value`, pinned at every cell by
  `rgb_dither.rs`'s `the_alpha_dither_path_reads_this_modules_tables`.

  So the alpha stage is now downstream of the disputed tile **by
  construction**, and `apply_alpha_dither`'s rounding
  (`(alpha & 7) > threshold`) reads the threshold directly, making the
  eight disputed cells observable in its output. The refusal is blocked on
  **D19** -- which Bayer arrangement the RDP uses -- which this audit
  itself scores UNKNOWN. Reclassify this row from *wgpu-side defect* to
  *blocked on D19*. Pinned by
  `the_alpha_dither_refusal_is_downstream_of_the_one_disputed_tile`, which
  kills a mutant reintroducing the duplicate.

  General lesson for this table: a row whose evidence is "two sites in one
  crate disagree" can be discharged by making them agree, which changes the
  verdict without anyone editing the refusal.


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

- **REVIEWED, KEPT, and blocked behind D1's *two* prerequisites** (see D1's
  outcome block). The row's own concession -- "refusing rather than guessing
  from one third of the bits is the *correct local* call" -- is right, and
  it is stronger than stated: wgpu recovers not one third of the bits but
  **none**, because the visible LSB on this lane holds `alpha >> 7` rather
  than a coverage MSB. `coverage_for`'s existing doc already derives the
  one case that *is* determined (a texrect's `pixel == 8` forces
  `sum >= 9 > 8`, so `wraps` is `true` regardless of the missing bits) and
  refuses only where the missing part is observable. No change.
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
