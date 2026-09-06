# Validating WM2000's frame 0 against an independent oracle

[`RT64-WM2000-REPLAY.md`](RT64-WM2000-REPLAY.md) established that WM2000's
real captured frame-0 packet executes end-to-end through `WgpuBackend` and
publishes 230,400 bytes into guest RDRAM. It closed with the honest caveat
that **no pixel was validated against anything**, and that a uniform `0xffff`
majority "is equally consistent with a texel decode that saturates".

This card took that measurement at `87925f36` and found a 99.38%
disagreement caused by one missing pipeline stage. It was **revised at
`3817911f`**: the port now runs that stage, the diff is re-measured, and
§3's original diagnosis of *why* the oracle produced two values is recorded
as disproved and replaced. It is **revised again here**: §1.1 closes the
comparison by controlling the one unmatchable term and reaches **0 differing
pixels**, and §4.2 diagnoses for the first time why entries 1-3 never ran.
Every number here comes from a run on this machine.

Companion docs: [`RT64-WM2000-REPLAY.md`](RT64-WM2000-REPLAY.md),
[`RT64-WM2000-CENSUS.md`](RT64-WM2000-CENSUS.md),
[`RT64-PORT-CARD-BRIEF.md`](RT64-PORT-CARD-BRIEF.md) ("Measure, never
assert").

---

## 1. Headline: with the one unmatchable term controlled, the two implementations agree exactly

**0 of 115,200 pixels differ** when `alpha_dither` is set to `Disabled` in
the shared word stream both backends decode. That is the validation result
this card exists to produce, and §1.1 states precisely what it does and does
not establish.

As captured — with `alpha_dither = Noise` — **100,235 of 115,200 pixels
differ (87.01%)**, down from 114,481 (99.38%) before the port ran the
blender. Every one of those 100,235 is a pixel where the oracle's *invented*
noise sequence rounded the blended alpha into the next five-bit bucket; §1.1
removes that single term and the disagreement goes to zero.

| | Port (`fn64-render-wgpu`) | Oracle (`fn64-render-reference`) |
|---|---|---|
| `0xffff` (R=G=B=31) | — (was 114,481) | — |
| `0xe739` (R=G=B=28) | — | 100,235 |
| `0xdef7` (R=G=B=27) | **114,481** | 14,246 |
| `0x0001` (fill) | 719 | 719 |

The 719 fill pixels still agree byte-for-byte, and **14,246 texrect pixels
now agree that previously did not**. Every remaining difference is a pixel
where the oracle's *noise alpha dither* rounded the blended alpha into the
next five-bit bucket.

**The residual is not reachable by a correct port.** WM2000's texrects latch
`alpha_dither = Noise` (other-mode high `0x0000acef`, bits 4:5 == 2). The
oracle's noise stream is SplitMix64 under a fixed arbitrary seed, and its own
source says what that is worth
(`crates/fn64-render-reference/src/raster/mod.rs:85-119`): the Programming
Manual "does not publish the hardware generator or its seed, so the
deterministic reference policy below is deliberately not described as the
silicon sequence", and SplitMix64 is used "without pretending to be the RDP's
unknown polynomial". §4 proves the dependence by control: changing only the
oracle's seed moves the split (100,235 / 99,912 / 100,253 / 100,310) while
the port's output does not move at all.

Reproducing that stream would transcribe an invented sequence, not implement
a documented stage. **Zero differing pixels is therefore not the right
target for the packet as captured**, and a port that reached it would have
copied the oracle rather than agreed with it.

---

### 1.1 Closing the gap: control the unmatchable term instead of transcribing it

The blocker was one *mode*, not one stage. `AlphaDither`
(`crates/fn64-render-wgpu/src/state.rs:147`) has four modes — `Pattern`,
`InversePattern`, `Noise`, `Disabled` — and **only `Noise` is unmatchable**.
So rather than copy the oracle's stream, this card holds that variable
fixed and compares everything else.

**The method, and why it is control rather than doctoring.**
`wm2000_packet_with_alpha_dither` rewrites other-mode high bits 4:5 in every
one of entry 0's 92 `SetOtherMode` commands, producing a new word stream
that differs from the capture in exactly those two bits per command and
nowhere else (asserted, not assumed: the helper checks every word is
unchanged outside the field). **That single rewritten stream is then handed
to BOTH implementations** — `wm2000_port_image` and
`wm2000_reference_image` receive the same `CapturedPacket`, unmodified.
Neither implementation was changed, and neither side sees a stream the other
does not. There is one word buffer and both backends decode it; that is what
structurally prevents this from being a tuning of one side.

**The result.**

| `alpha_dither` | wire encoding | port | oracle | differing pixels |
|---|---|---|---|---|
| `Noise` (as captured) | 2 | `0xdef7` × 114,481 + `0x0001` × 719 | `0xe739` × 100,235 / `0xdef7` × 14,246 / `0x0001` × 719 | **100,235** |
| `Disabled` | 3 | `0xdef7` × 114,481 + `0x0001` × 719 | `0xdef7` × 114,481 + `0x0001` × 719 | **0** |
| `Pattern` | 0 | *refused by name* | — | **not compared** |
| `InversePattern` | 1 | *refused by name* | — | **not compared** |

**`Pattern` yields no number, and the refusal is the finding.** `Pattern`
and `InversePattern` both resolve to an ordered tile, substituting `Bayer`
when RGB dither is `Disabled` — which is exactly what this packet latches
(other-mode high `0xef00acef`, bits 6:7 == 3; the substitution arm is
`targets/texrect.rs:1407`, which covers both ordered modes jointly). The
port refuses `Bayer` by name:

> `G_MDSFT_ALPHADITHER selects the ordered Bayer dither tile, whose
> threshold and arithmetic this crate's RT64 and reference ports disagree
> about; no evidence in this repo settles which is the RDP's`

That refusal protects a real, already-pinned disagreement (8 of 16 cells,
`rgb_dither.rs`'s own
`bayer_matrix_disagrees_with_reference_oracle_at_documented_cells`).
Producing a `Pattern` pixel count would have required weakening it, so no
count is reported. **`Disabled` is the only reachable deterministic mode for
this packet, and it is the one that carries the validation.**

**The expectation was hand-derived, not captured.** The texrects' combiner
is flat `Primitive` with prim alpha `0xdf` = 223 (§3), blended over a zero
destination, so the composite is `255 × 223/255 = 223`, whose five-bit
channel is `223 >> 3 = 27` → `0xdef7`. With dither disabled *both* sides
must produce exactly that, and both do; the 719 fill pixels stay `0x0001`.
The two histograms are asserted independently, so a defect moving both sides
the same way could not pass as agreement.

**What this validates.** The port's blender, combiner, tile addressing,
texel-fetch path as this packet exercises it, coverage handling and
framebuffer write-back agree **byte-for-byte** with a genuinely independent
CPU rasterizer over WM2000's real captured frame-0 bytes. What it does not
validate is listed in §7 and unchanged: this packet's combiner reads no
texel, so texture sampling is still untested, and the noise term itself
remains unestablished on either side.

---

## 2. The oracle, and why

**`fn64-render-reference`** — the deterministic software rasterizer, fn64's
headless comparison oracle by its own lib doc
(`crates/fn64-render-reference/src/lib.rs:1`).

- **Genuinely independent.** A CPU rasterizer with its own GBI/RDP decoder
  (`gbi/stream.rs`), its own TMEM model (`gbi/state.rs`), its own combiner
  (`raster/combiner.rs:40`) and its own framebuffer writeback
  (`backend/framebuffer_io.rs:123`). It shares no rendering code with
  `fn64-render-wgpu`.
- **Same seam, same bytes.** Both backends implement `fn64_render::RenderBackend`.
  The oracle is driven through `process_rdp_commands`
  (`backend/render_backend.rs:106`), which reads RDP words out of a guest
  RDRAM buffer and writes the color image back into it — the same shape as
  the port's `dispatch_dpc_submission` path.
- **It already handles this packet's shape.** All 19 of entry 0's opcodes
  have decode arms, including `LoadTLUT` (`gbi/stream.rs:1170`), `LoadBlock`
  (`:1193`) and `G_TEXRECT` (`:1324`).

**`fn64-render-conformance` was NOT reusable.** Its own README states it
plainly: "No such RT64 or Rust-port runner is registered yet, so every
backend row stays open," and `BackendProducedObservable` has private fields
and no public constructor by design. It is a receipt-issuing ladder for
closed rows, not a two-backend differ. Using it would have meant registering
a reviewed runner — a much larger, different card.

**`fn64-render-rt64`** was not used: FFI-bound and heavier to drive
headlessly, and unnecessary once a same-repo independent oracle was
available.

---

## 3. The disagreement, diagnosed — and the first diagnosis corrected

**The texrects' combiner reads no texel at all.** The state latched
immediately before the 60 texrects (packet offset `0x0960`) is
`SetCombine 0xfcffffff / 0xfffdf6fb`. Decoded through `CombineParams`' own
second-cycle bit positions, both slices are
`(Zero − Zero) * Zero + Primitive` — flat `Primitive`. The `SetPrimColor` at
`0x0968` is `0xffffffdf`: RGB 255,255,255, alpha `0xdf` = 223.

**The blender program.** Other-mode low `0x005041c8` gives cycle 1
`P = Combined, A = Combined, M = Framebuffer, B = 1 − A`, with `FORCE_BL`
and `IM_RD` set and `AA_EN` set. `blend_fragment`'s `M == Framebuffer` arm
makes the composite `combined * (223/255) + destination * (1 − 223/255)`.

**The destination is zero everywhere.** The 60 fills are Fill-cycle with
fill colour `0x00010001`, whose `decode_16` is RGB `[0, 0, 0]` with the
coverage bit set. So the composite is `255 * 223/255 + 0` = 223 → 5-bit 27
→ `0xdef7`, which is exactly what the port now publishes on all 114,481
texrect pixels.

**A correction, recorded rather than quietly replaced.** This card's first
version derived the oracle's two values from *destination content* — 0 for
"poison-cleared" pixels and 8 for the `0x0001` fill — and landed on 223/224,
which happens to fall in the same two five-bit buckets. That derivation is
**wrong**, and the measurement that disproves it is direct: replaying the
packet with the 60 texrects removed publishes a uniform `0x0001` across all
115,200 pixels on *both* backends, so no pixel has a non-zero destination
for the texrects to blend against. The two values are also scattered
pseudo-randomly rather than following the fill geometry (row 0's first
sixteen pixels are `e739 e739 e739 e739 def7 def7 e739 …`), which no
destination-content explanation predicts.

**What actually splits them is noise alpha dither.** Other-mode high
`0x0000acef` selects `rgb_dither = Disabled` but `alpha_dither = Noise`.
The oracle perturbs the combined alpha before blending
(`raster/draw.rs:606-612`) via `apply_alpha_dither`, whose arithmetic is
`(alpha >> 3) + ((alpha & 7) > threshold)` re-expanded by
`(five << 3) | (five >> 2)`. With `alpha = 223`, `alpha & 7 == 7`, so the
comparison is true for every 3-bit threshold except 7:

| noise threshold | five | dithered alpha | blended | 5-bit | value |
|---|---|---|---|---|---|
| 0–6 (7 cases in 8) | 28 | 231 | 231.0 | 28 | `0xe739` |
| 7 (1 case in 8) | 27 | 222 | 222.0 | 27 | `0xdef7` |

Predicted 7/8 : 1/8 of 114,481 is **100,171 : 14,310**; measured is
**100,235 : 14,246** — the expected fluctuation of a pseudo-random stream
over that many samples, and a 0.06% agreement with a ratio nothing was
fitted to.

**Which side is right.** On the blender, the oracle was, and the port now
matches it: `FORCE_BL` is set, so the blender is unconditionally active, and
a prim alpha of 223 is the game asking for an 87.5%-opacity overlay. On the
dither, **neither side is established as right**, because the quantity in
dispute is an unpublished hardware noise sequence. The port applies no
dither and says so; the oracle applies a stream it declares non-silicon.

**Not claimed:** that either implementation is hardware-correct. Nothing
here is checked against an N64.

One further caveat, from the oracle's side: its RGBA16 LSB is the high bit
of *stored coverage*, not pixel alpha (`backend/framebuffer_io.rs:120-122`).
That affects bit 0 only. It is not the cause of the residual — both values
in dispute carry LSB 1.

---

## 4. The seed control, and the palette-invariance probe

### 4.0 The residual tracks the oracle's seed, not the packet

Replaying the identical captured bytes through the oracle at four different
noise seeds, with nothing else changed and neither implementation modified:

| oracle noise seed | `0xe739` | `0xdef7` |
|---|---|---|
| `0x4e36345244504e53` (default) | 100,235 | 14,246 |
| `0x1` | 99,912 | 14,569 |
| `0x2` | 100,253 | 14,228 |
| `0xdeadbeef` | 100,310 | 14,171 |

The port's output is `0xdef7` × 114,481 in every one of these runs. The
split is a property of the oracle's seed; the packet does not mention it.
Every seed still lands within 0.4% of the 7/8 the dither arithmetic
predicts, which is what makes them the same mechanism rather than noise in
the measurement.

### 4.1 The palette-invariance probe: refuted, via its own control

The brief's hypothesis was that `0xffff` uniformity meant the texel path was
not really sampling the TLUT. **Measured, it is not the explanation.**

The packet's own TLUT source region (`0x00100660`, from its
`SetTextureImage`) was overwritten with three palettes — `0x0000`, `0xf801`
(red), `0x07c1` (green) — and the packet replayed:

| Palette | Port output | Oracle output |
|---|---|---|
| `0x0000` | `0xffff` × 114,481 + `0x0001` × 719 | `0xe739`/`0xdef7`/`0x0001` |
| `0xf801` | identical | identical |
| `0x07c1` | identical | identical |

The port is invariant. **So is the oracle** — and that control is what makes
the finding honest. A one-sided probe would have read as "the port is not
sampling"; the control shows the palette region is not consulted by *either*
implementation, because §3's flat-`Primitive` combiner reads no texel.

The same perturbation applied to the CI4 **texture data** (the `LoadBlock`
source at `0x001006e0`, filled with `0x00` and `0xff`) also moved neither
side: 0 pixels changed on both.

**Answer to "is the texel path really sampling?"** — For this packet the
question does not arise: the program has no texel term. The probe therefore
neither confirms nor refutes the port's sampler, and this card makes no
claim about it. A packet whose combiner actually reads `Texel0` is required
to test that, and entry 0 is not one.

An independent read of the port's sampler found the TLUT chain intact and
consumed (`tmem/read.rs:431-440`, `read_canonical_tlut_entry` at `:552`),
with no clamp-to-white or unsupported-format-to-white branch reachable. That
is consistent with the measurement but is a source reading, not a
measurement, and is recorded as such.

---

## 4.2 Entries 1-3: diagnosed, and the refusal protects something real

Entries 1-3 had never been compared. They stop before any pixel, at
`crates/fn64-render-wgpu/src/raw_dpc/production_adapter.rs:1224`:

```
assert_eq!(source_accesses.len(), 1,
           "v11's admitted TMEM source plan is exactly one journal access wide")
```

measured `left: 49`, `right: 1`. **Diagnosed here for the first time, and
NOT widened** — the assert is a truthful guard on a real invariant, not an
arbitrary scope limit.

**Where 49 comes from.** Entry 0 uses only `LoadBlock` and `LoadTLUT`, both
of which read one contiguous RDRAM range. Entries 1-3 additionally use
**`LoadTile`** (25 of them), which entry 0 never does. A `LoadTile` reads a
2D sub-rectangle row by row, and each row is a separate, discontiguous
RDRAM range — one journal access each. Hand-derived from the wire
coordinates of the first one (`uls=0 ult=0 lrs=128 lrt=192`, 10.2 fixed
point): T runs 0..=48, which is **49 rows, so 49 source accesses**. The
decoder is correct to produce them; `tmem/wire.rs:305-317` builds exactly
one source range per row, bounded by `MAX_RESOURCE_ACCESSES`.

**What the assert protects: journal-order determinism and publication
identity.** Two independent mechanisms, both real:

1. **`finish` compares position by position.**
   `ExactRawDpcPlanWriter::push_tmem_load` pushes exactly **two** accesses,
   `source` then `destination` (`fn64-render/src/render_ir.rs:2483-2487`),
   while the decoder's journal orders them *all sources, then all
   destinations* — `first_destination_access = first_access_index +
   source.access_count()` (`tmem/wire.rs:544`). With 49 sources the writer
   would emit `[src0, dst0, …]` where the journal has `[src0, src1, …]`:
   both short and misordered. `finish` rejects exactly this
   (`render_ir.rs`'s own `finish_rejects_a_journal_missing_an_access…`,
   `…_with_an_extra_access…`, `…_a_reordered_journal_even_with_the_same_access_set`).
2. **`source_access_count` is part of a published content digest.** It is
   hardcoded `1` on the neutral production path
   (`tmem/physical.rs:1425`), cross-checked against the decoder-derived
   `plan.source().access_count()` (`:1340`) by a field-by-field equivalence
   check that names `"source access count"` on mismatch (`:1540`), and fed
   into `proposal_identity`'s projection bytes (`:1829`). Widening it
   changes a publication identity, not just an adapter local.

**Measured, not reasoned.** Removing only the assert (experiment, reverted)
does not produce pixels — it produces the next honest refusal:

> `raw-DPC plan seal failed: raw-DPC plan writer accumulated access count
> is 1575; exact journal requires 2790`

The 1,215-access gap hand-derives exactly from the wire: entry 1's 25
`LoadTile`s split 10 at 49 rows and 15 at 50 rows, so the dropped accesses
are `10 × 48 + 15 × 49 = 1215`. Two independent instruments — the runtime's
own count and a hand walk of the captured coordinates — agree to the access.

**Verdict: this is a genuine invariant and it stays.** Reaching entries 1-3
is a real port task (teach the writer and the neutral `TmemLoadSemantics`
to carry an N-access source run, and re-derive the affected publication
digest), not an assert to relax. Their pixel diffs are therefore **not
measured**, and this card reports that rather than a number.

---

## 5. Commands

Capture (unchanged, see [`RT64-WM2000-REPLAY.md`](RT64-WM2000-REPLAY.md) §2).
The fixture used here is the corrected capture: **426 entry-0 rows, 3,408
wire bytes**.

Those two counts identify it and an operator can check them against their
own dump. A sha256 was cited here as well, and is deliberately not any
more: the capture is a game's own RDP command words, so it cannot be
committed (`README.md`'s no-game-content rule) and no test can gate the
hash. An ungated hash is a claim about a file nobody can verify, which is
exactly what `lint-docs.py`'s content-hash rule exists to stop -- it had
been failing on this line since before this branch.

```sh
FN64_WM2000_PACKET_TSV=<scratch>/packet.tsv \
FN64_WM2000_PACKET_ENTRY=0 \
  cargo nextest run -p fn64-abi --offline -E 'test(wm2000)' --no-capture
```

The comparison and its probes live beside the existing replay test in
`crates/fn64-abi/src/task_dispatch/tests/raw_dpc_session_integration.rs`:

| Test | What it measures |
|---|---|
| `wm2000_frame_zero_compared_against_the_reference_rasterizer` | the byte-for-byte diff, reported not asserted |
| `wm2000_frame_zero_blender_runs_and_the_residual_is_oracle_dither` | pins both sides' images, derives the blend and the dither arithmetic, and asserts the 719 fill pixels separately |
| `wm2000_frame_zero_oracle_split_is_a_property_of_its_noise_seed` | the seed control that makes the residual a finding, not a defect |
| `wm2000_frame_zero_palette_invariance_probe` | the port's TLUT invariance |
| `wm2000_frame_zero_palette_invariance_reference_control` | the oracle's, which refutes the one-sided reading |
| `wm2000_frame_zero_texture_data_sensitivity_probe` | both sides vs. the CI4 texture data |
| `wm2000_frame_zero_combiner_constant_probe` | how each side moves under rewritten `SetEnvColor`/`SetPrimColor` |
| `wm2000_frame_zero_primitive_color_response_sweep` | that the port *does* honour `SetPrimColor` |
| `wm2000_frame_zero_agrees_exactly_when_alpha_dither_is_disabled` | **§1.1's headline: 0 differing pixels with the unmatchable term controlled** |
| `wm2000_frame_zero_pattern_alpha_dither_is_refused_by_name_not_compared` | that `Pattern` routes to the refused Bayer tile, so it yields no count |
| `the_noise_free_comparison_detects_a_single_perturbed_pixel` | that the *controlled* comparison can fail |
| `the_wm2000_image_comparator_detects_a_single_perturbed_pixel` | the comparator can fail |
| `a_trimmed_wm2000_packet_fails_the_census_identity_control` | a stand-in is rejected |

Both sides are read back through `RdramView::copy_logical_bytes`, so the
comparison is logical-vs-logical. The oracle writes its color image through
`RdramViewMut` (`backend/framebuffer_io.rs:163-188`), the same authority, so
no `^3`/`^2` lane mismatch can masquerade as a content difference.

**One harness accommodation, disclosed.** WM2000's captured stream ends with
its own `G_ENDDL` (`0xdf000000`), and `process_rdp_commands` appends a
terminator of its own at `end` (`backend/render_backend.rs:137-138`). The
oracle refuses `0xdf` as an *opcode* by name. The last command is therefore
excluded from the decoded range and the oracle's own terminator lands at
exactly the same address — the byte stream the oracle decodes is the
packet's, unmodified. Without this the oracle refuses at
`raw RDP opcode G_ENDDL (0xdf, wire byte 0xdf) at 0x00001d48 is unsupported`,
which is a harness artifact and not a port finding.

---

## 6. Verification

- **10 consecutive identical comparison runs.** All ten report the same
  three histogram lines and the same 100,235-pixel diff as captured, and
  all ten report **0 differing pixels** under §1.1's `Disabled` control.
- **§1.1's comparison is mutation-tested: 5 mutants, 4 killed, 1 proven
  equivalent.**
  - *Killed*: making the mode rewrite a no-op (all 3 controlled tests);
    rewriting only the first `SetOtherMode` so the texrects' own latch is
    untouched (all 3); setting the control to `Noise` instead of `Disabled`
    (1); inverting the comparator's `!=` to `==` (1).
  - *Equivalent, with proof*: perturbing the oracle side instead of the port
    side. Byte inequality is symmetric, so the two forms are the same test;
    it is recorded as equivalent rather than counted as a kill.
  - *Positive control on the fixture*: a trimmed fixture is rejected by the
    366-command census assertion before any comparison runs, so the zero
    cannot come from a degenerate stand-in.
- **Mutation-tested, 16 mutants, 15 kills, 1 proven equivalent** (8 on the
  blender, 8 on §8's stages).
  - *Killed*: skip the blend stage (3 tests); swap the blend source and
    destination operands (15); truncate instead of round in the composite
    (8); apply a blend to the fill executor too (19) — the fills already
    agreed byte-for-byte, so this had to fail; drop `read_pixel`'s 5-bit
    low-bit replication (1); widen the `FORCE_BL` refusal back over the
    `AA_EN`-clear case (6); hardcode `blend_enabled = true` (1).
  - *Two mutants first survived, and the reach gaps they exposed are the
    deliverable.* Skipping the blend stage was initially caught only by the
    whole-image comparison, because the crate-local test exercised the
    helper rather than the pixel loop; the composition is now a named
    `blend_and_write_pixel` the test calls, matching the precedent
    `combine_one_texel`'s own doc records. Dropping the `>> 2` expansion
    survived a round-trip test, because `write_pixel`'s `>> 3` recovers the
    same five bits either way; it is now pinned against the fill
    executor's own decode (5-bit 27 → 222, not 216).
  - *Stage mutants, all killed*: skip the alpha-compare gate; make it
    strict (`>` for `>=`) at the threshold boundary; skip coverage-to-alpha;
    swap `CVG_X_ALPHA` with `ALPHA_CVG_SEL`; skip alpha dither; move the
    noise-dither threshold off its proven endpoint; admit the reserved
    `G_AC` encoding; admit `cvg_dst = Save`.
  - *Two more reach gaps found and fixed the same way.* Skipping
    coverage-to-alpha survived a `CVG_X_ALPHA`-with-zero-alpha witness,
    because a zero alpha makes the blend a pure destination pass-through and
    the stored halfword was unchanged either way; `ALPHA_CVG_SEL` separates
    them (5-bit 31 with the stage, 8 without). Skipping alpha dither
    survived until the ordered `MagicSquare` arm was exercised **through the
    pixel loop** rather than through `apply_alpha_dither` alone.
  - *Equivalent, with proof*: reading the blend destination from the
    caller's `resident_bytes` instead of the buffer being written. `offset`
    is injective in `(row, column)` over the loop's range, so each byte
    range is written at most once per call and the two reads are provably
    identical; cross-command composition is preserved by the caller, which
    threads each command's full-extent output in as the next one's
    `resident_bytes` (`production.rs`'s own "the accumulation is the
    composition" note). The working-buffer form is kept as the more robust
    one, not because it is observably different here.
- **Positive control on the fixture**: the census identity assertions (366
  commands, 19 opcodes, 60 `G_FILLRECT`, 60 `G_TEXRECT`) run before the
  comparison, and a fixture trimmed to 3/4 of its entry-0 rows was measured
  as failing them.
- **Workspace**: 8,294 passed / 13 skipped before, **8,307 after the
  blender** and **8,319 after §8's three stages** (twenty-five new tests
  in total). Measured in this worktree at `3817911f`, not quoted.
  §1.1/§4.2 add three more: **8,319 before and 8,322 after**, measured in a
  worktree at `1020f1d0` before any change and again after, not quoted.
- **Release profile** (`RUSTFLAGS="-C debug-assertions=off"`): 8,294 before
  and 8,319 after, identical to debug in both directions; §1.1/§4.2's run
  measures **8,322 in release, identical to debug**, and the workspace is
  green both with and without the packet fixture in the environment. The test *set* is
  also identical across profiles (`cargo nextest list`, 2,626 lines each,
  diff empty apart from the build-time line), so no count here is
  profile-dependent.
- **`scripts/lint-docs.py`**: 3 warnings and 1 error before, the same 3 and
  the same 1 after — measured in a clean baseline worktree at `3817911f`
  and in this one. The error is the pre-existing "content hash that no test
  checks" on this file's own §5 citation; it is a baseline to preserve, not
  a regression introduced here.
- **Dead code**: 1,060 `never used` / 1,218 including `never read` and
  `never constructed`, unchanged before and after, both measured with
  `cargo check --workspace --all-targets` — the baseline in a separate
  clean worktree. The circulating 1,201 did not reproduce under either
  recipe. **Re-measured independently at `1020f1d0`: the same 1,060 /
  1,218**, so the circulating 1,041 and 1,218 figures resolve to this pair
  and 1,201 remains unreproduced.
- **Host-GPU tests, and a prior lane's finding corrected.**
  `wgpu_backend_draws_a_real_texture_rectangle_at_its_own_wire_position` is
  red under `--features host-gpu-tests` on a clean baseline worktree at
  `1020f1d0` **and** on this one — pre-existing, verified rather than
  inherited. **The earlier "`flip_wire_position` matches no test in this
  checkout" is withdrawn**: the bare string does not appear, but the test it
  referred to does, under the fuller name
  `wgpu_backend_draws_a_real_texture_rectangle_flip_at_the_same_wire_position`,
  and it is **also red** — pre-existing at `1020f1d0` by the same clean-
  worktree check. Both halves of the briefed red pair are therefore real;
  the earlier report was a name-matching artifact, not a measurement.

---

## 7. Verdict: does WM2000's frame 0 render correctly through the Rust port?

### 7.1 The scoreboard, per entry

| Entry | Executes? | Differing pixels, as captured | Differing pixels, `alpha_dither = Disabled` |
|---|---|---|---|
| **0** | **yes**, all 366 commands | **100,235** of 115,200 (87.01%) | **0** of 115,200 |
| 1 | **no** — refused in the adapter (§4.2) | not reached | not reached |
| 2 | **no** — same refusal | not reached | not reached |
| 3 | **no** — same refusal | same | not reached |

Entries 1-3 all carry `LoadTile`, which entry 0 does not, and stop at the
one-source-access invariant §4.2 diagnoses. That refusal is real and was
not weakened, so their columns stay empty rather than being filled with a
number obtained by relaxing a guard.

### 7.2 The verdict

**For entry 0, yes — with one named term excluded, and that exclusion is
controlled rather than assumed.**

Fed WM2000's real captured frame-0 bytes, the Rust port and a genuinely
independent CPU rasterizer publish **byte-identical** 115,200-pixel images
once `alpha_dither` is set to `Disabled` in the single word stream they both
decode. Zero pixels differ. That is a stronger result than this card could
previously state, and it is the result the 100,235 residual was hiding: the
residual was never the port's, and now it is shown to be *only* the invented
term, because removing that term removes the entire disagreement.

**What is now validated** (entry 0, and only entry 0): the blender's
`M = Framebuffer` arm, the flat-`Primitive` combiner, tile addressing, the
fill path, coverage handling, and framebuffer write-back — all
byte-for-byte against an independent implementation.

**What is still NOT validated**, unchanged and restated so the zero is not
over-read:

- **Texture sampling.** Entry 0's combiner has no texel term (§4.1), so
  nothing here exercises the TMEM/TLUT sampler.
- **The noise dither itself.** Neither side is established as correct; the
  quantity is an unpublished hardware sequence, and this card excludes it
  rather than settling it.
- **Alpha compare and the coverage-alpha interaction.** Inert for this
  packet (§8) — their evidence is characterization plus mutation testing,
  not this differential.
- **RGB dither.** Deliberately not ported; the two ports disagree on both
  table and arithmetic (§8).
- **Ordered alpha dither.** Refused by name for this packet's
  `Bayer` substitution (§1.1).
- **Entries 1-3, triangles, and every other title.** Entry 0 is
  triangle-free; the census measured 152 of 218 frames as carrying
  triangles.
- **Hardware.** No comparison against an N64 was made, on any stage.

**What would settle the rest**: (a) hardware or an independent emulator for
the RDP's actual noise sequence and the Bayer tile; (b) a packet whose
combiner reads a texel, to validate the sampler; and (c) the N-source-access
port work §4.2 scopes, to reach entries 1-3.

What this does and does not cover: **one entry, one frame, one title's
boot/logo window**. Entry 0 is triangle-free and its texrect program reads
no texel, so this validates the fill path, the constant-color path and now
the blender's `M = Framebuffer` arm, and says nothing about texture
sampling, triangles, or the 152 of 218 frames the census measured as
carrying triangles. Entries 1–3 were not compared.

---

## 8. The other three post-combiner stages, and why this comparison does not validate them

A follow-on commit wired **alpha compare, alpha dither and coverage** into
the same executor, so three of the four stages its header declared absent
now run. The pixel diff **did not move**: still 100,235 as captured. §1.1's
controlled comparison does not change that either — it removes the alpha
dither term rather than exercising it, so the three stages below remain
unvalidated by this differential for exactly the reasons stated here.

That is the expected result, and it is the reason this section exists rather
than a claim of broader validation. Measured across **all four captured
entries** (315 texrects), every one latches the identical mode:

| stage | latched value | exercised here? |
|---|---|---|
| alpha compare | `G_AC_NONE` (low bits 0:1 = 0) | **no** — the gate always passes |
| `CVG_X_ALPHA` / `ALPHA_CVG_SEL` | both clear | **no** — coverage never touches colour |
| alpha dither | `Noise` (high bits 4:5 = 2) | yes, and it is §1's residual |
| RGB dither | `Disabled` (high bits 6:7 = 3) | no |

**So this oracle comparison would not detect a defect in the alpha-compare
gate or the coverage-alpha interaction at all.** Their evidence is
hand-derived characterization plus mutation testing
(`fragment_stage_tests`), not this differential, and the card says so rather
than letting one number imply four validations.

**Two deliberate non-ports, both blocked on evidence rather than effort.**

- **RGB dither is not run.** The workspace's two ports disagree on the Bayer
  table at 8 of 16 cells — already pinned by `rgb_dither.rs`'s own
  `bayer_matrix_disagrees_with_reference_oracle_at_documented_cells` — and
  on the arithmetic at every input where `(channel & 7)` straddles the
  threshold: RT64 computes `min(channel + threshold, 255) >> 3`, the
  reference `if (channel & 7) > threshold { (channel & !7) + 8 }`. Witness:
  channel 1 at threshold 0 gives 5-bit 0 under one and 1 under the other.
  Refusing outright was measured and rejected — encoding 0 is `MagicSquare`,
  the power-on default this crate's own fixtures latch, so a refusal would
  decline packets that execute correctly today.
- **`G_AC_DITHER` is refused by name.** A gate has no bounded-interval
  argument, so no endpoint substitutes for the missing sequence.

**The `Noise` dither modes run at a proven endpoint, not a guess.** Over all
256 alpha values the mode's output set is exactly `{floor, floor + 1}` in
the five-bit channel — asserted exhaustively — and the maximum 3-bit
threshold selects `floor`. The executor's constant is therefore a member of
the mode's real output range rather than a third value between the two.
This is disclosed in the module's Nonclaims, not presented as parity.

**The `blend_cycle_count` hazard, settled.**
`rt64_blender_analysis::blend_cycle_count` and
`BlendModeState::cycle_count` disagree numerically for every
non-`FORCE_BL` mode, and **neither is wrong**: the former counts cycles that
*actually blend* (its consumers are the `uses_*` predicates, and a bypassed
last cycle reads only `P`), the latter counts *loop iterations* (the loop
handles the bypass internally and must still visit the cycle to resolve
`P`). Both are faithful ports of differently-purposed upstream functions.
Pinned, with the reconciliation, in
`the_two_cycle_counts_disagree_by_design_and_the_reason_is_pinned`. It is
unreachable for this packet in any case — `FORCE_BL` is set, where the two
agree.

---

**Nonclaims.** The oracle was not adjusted, and neither implementation was
tuned toward the other. **§1.1's controlled comparison rewrites two bits of
the packet's own other-mode word and feeds the identical rewritten stream to
both implementations** — one word buffer, both backends decoding it — so it
is a held variable, not a tuned side; the zero it reports is a result for
that controlled input and is not claimed for the packet as captured. No hardware comparison was made and no claim of
hardware correctness is made for either side, on any stage. **RGB dither is
not implemented** and is declared so in the executor's header; alpha
compare, alpha dither and coverage are implemented but **not validated by
this comparison** (§8). The port's TMEM/TLUT sampler is neither validated
nor faulted — this packet does not exercise it. No claim is made about
entries 1–3, about frames carrying triangles, or about any other title. No
`repr(C)`, size, alignment or ABI claim is made.
