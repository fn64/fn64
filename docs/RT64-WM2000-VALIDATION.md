# Validating WM2000's frame 0 against an independent oracle

[`RT64-WM2000-REPLAY.md`](RT64-WM2000-REPLAY.md) established that WM2000's
real captured frame-0 packet executes end-to-end through `WgpuBackend` and
publishes 230,400 bytes into guest RDRAM. It closed with the honest caveat
that **no pixel was validated against anything**, and that a uniform `0xffff`
majority "is equally consistent with a texel decode that saturates".

This card takes that measurement. Every number here comes from a run on this
machine, at `87925f36`.

Companion docs: [`RT64-WM2000-REPLAY.md`](RT64-WM2000-REPLAY.md),
[`RT64-WM2000-CENSUS.md`](RT64-WM2000-CENSUS.md),
[`RT64-PORT-CARD-BRIEF.md`](RT64-PORT-CARD-BRIEF.md) ("Measure, never
assert").

---

## 1. Headline: the two implementations DISAGREE, and the port is the one at fault

**114,481 of 115,200 pixels differ — 99.38%.** The 719 fill pixels agree
byte-for-byte; every one of the 60 texrects' pixels disagrees.

| | Port (`fn64-render-wgpu`) | Oracle (`fn64-render-reference`) |
|---|---|---|
| `0xffff` (R=G=B=31) | 114,481 | — |
| `0xe739` (R=G=B=28) | — | 100,235 |
| `0xdef7` (R=G=B=27) | — | 14,246 |
| `0x0001` (fill) | 719 | 719 |

`100,235 + 14,246 = 114,481` exactly: the oracle resolves the same pixel set
into **two** structured values where the port emits one flat value.

**The port is wrong, and the cause is named**: the port does not run the
blender. Its own module doc says so — `targets/texrect.rs:76-77`, "**No
blending, alpha compare, dither, or coverage.**" The oracle runs it. §3
derives both sides' values from the packet's own words and reproduces them
exactly.

**The `0xffff` saturation suspicion is refuted** (§4), but not in the port's
favour: `0xffff` is the arithmetically *correct* combiner output, written
without the blending stage that must follow it.

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

## 3. The disagreement, diagnosed

**The texrects' combiner reads no texel at all.** The state latched
immediately before the 60 texrects (packet offset `0x0960`) is
`SetCombine 0xfcffffff / 0xfffdf6fb`, which decodes to
`(Zero - Zero) * Zero + Primitive` — flat `Primitive`. The `SetPrimColor` at
`0x0968` is `0xffffffdf`: RGB 255,255,255, **alpha `0xdf` = 223**.

That single fact explains both sides' behaviour:

- **Port**: writes the combiner output straight to the destination. RGB 255
  → 5-bit 31 → `0xffff`. Correct combiner arithmetic, no blender.
- **Oracle**: blends under the latched other-mode low word `0x005041c8`
  (`FORCE_BL`, `IM_RD`, `AA_EN`, `CLR_ON_CVG`, `cvg_dst=Wrap`). White at
  α = 223/255 over the destination gives
  `255 * 223/255 + dst * (1 - 223/255)`:

  | destination | blended | 5-bit | value |
  |---|---|---|---|
  | 0 (poison-cleared) | 223.0 | 27 | `0xdef7` |
  | 8 (the `0x0001` fill) | 224.0 | 28 | `0xe739` |

  Both are reproduced exactly, and the derivation is asserted in the pin
  test rather than read off either implementation.

**Which side is right.** The oracle. `FORCE_BL` is set, so the blender is
unconditionally active for these texrects; a prim alpha of 223 is not
incidental, it is the game asking for an 87.5%-opacity overlay. The port
skipping that stage is a documented gap in its own module header, not a
disputed reading. This is a **missing stage**, not an arithmetic error —
the port's combiner output is right and is then published unmodified.

**Not claimed:** that the oracle is hardware-correct. Nothing here is
checked against an N64. What is proven is that two independent
implementations of the same documented pipeline disagree, that the
difference is exactly one pipeline stage, and that the stage is one the port
declares it does not implement.

One further caveat, from the oracle's side: its RGBA16 LSB is the high bit
of *stored coverage*, not pixel alpha (`backend/framebuffer_io.rs:120-122`).
That affects bit 0 only and cannot account for a 3-to-4 step in the 5-bit
color channels.

---

## 4. The palette-invariance probe: refuted, via its own control

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

## 5. Commands

Capture (unchanged, see [`RT64-WM2000-REPLAY.md`](RT64-WM2000-REPLAY.md) §2).
The fixture used here is the corrected capture, sha256
`a35515bca662ce9d1b007300553484d28650ae1c238c316b00399033bbe0650e`, 426
entry-0 rows, 3,408 wire bytes.

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
| `wm2000_frame_zero_port_omits_the_blender_the_oracle_runs` | pins both sides' images and derives the blend arithmetic |
| `wm2000_frame_zero_palette_invariance_probe` | the port's TLUT invariance |
| `wm2000_frame_zero_palette_invariance_reference_control` | the oracle's, which refutes the one-sided reading |
| `wm2000_frame_zero_texture_data_sensitivity_probe` | both sides vs. the CI4 texture data |
| `wm2000_frame_zero_combiner_constant_probe` | how each side moves under rewritten `SetEnvColor`/`SetPrimColor` |
| `wm2000_frame_zero_primitive_color_response_sweep` | that the port *does* honour `SetPrimColor` |
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
  three histogram lines and the same 114,481-pixel diff.
- **Mutation-tested, 5 mutants, 5 kills.** Port histogram `0xffff`→`0xfffe`;
  oracle count `100_235`→`100_236`; diff count `114_481`→`114_480`; blend
  derivation `27`→`28`; and — the load-bearing one — **substituting the port
  for the oracle**, which fails, proving the two sides are genuinely
  different code paths and not the same backend read twice.
- **Positive control on the fixture**: the census identity assertions (366
  commands, 19 opcodes, 60 `G_FILLRECT`, 60 `G_TEXRECT`) run before the
  comparison, and a trimmed packet is measured as failing them.
- **Workspace**: 8285 passed / 13 skipped before, 8294 / 13 after (nine new
  tests). Measured in this worktree, not quoted.
- **Release profile** (`RUSTFLAGS="-C debug-assertions=off"`): same counts.
- **`scripts/lint-docs.py`**: clean, 3 pre-existing warnings before and
  after, unchanged.
- **Dead code**: 1,060 `never used` / 1,218 including `never read` and
  `never constructed`, unchanged before and after. The brief's 1,201 did not
  reproduce under either recipe in a fresh worktree; both figures are
  reported with the recipe that produced them rather than reconciled to a
  quoted number.
- Two host-GPU tests (`texture_rectangle_at`, `flip_wire_position`) are
  pre-existing red and were not run in this lane's default profile; no blame
  is inherited or claimed.

---

## 7. Verdict: does WM2000's frame 0 render correctly through the Rust port?

**No.** 99.38% of its pixels are wrong, by a named and reproduced mechanism.

The bar the card was set — "prove those bytes are right" — is **met as a
measurement and failed as a result**. That is the more useful outcome: the
port's frame 0 was previously "two distinct values, unvalidated"; it is now
"one missing pipeline stage, quantified to the pixel, with the correct
values known".

What this does and does not cover: **one entry, one frame, one title's
boot/logo window**. Entry 0 is triangle-free and its texrect program reads
no texel, so this validates the fill path and the constant-color path and
says nothing about texture sampling, triangles, or the 152 of 218 frames the
census measured as carrying triangles. Entries 1–3 still stop at the v11
access-count frontier and were not compared.

What remains, precisely:

1. **Implement the blender for the CPU texrect path**, or route texrects
   through a path that has one. This is the whole of the measured
   disagreement. It is not a one-line fix and was not attempted here.
2. **Re-run this comparison** once it lands; agreement then is a real
   validation of frame 0.
3. **Find a packet whose combiner reads `Texel0`** to validate the sampler
   at all — entry 0 cannot, and the palette probe's inconclusiveness on that
   question is a gap this card records and did not fill.

**Nonclaims.** Neither implementation was adjusted to make them agree. No
hardware comparison was made and no claim of hardware correctness is made
for either side. The port's TMEM/TLUT sampler is neither validated nor
faulted — this packet does not exercise it. No claim is made about entries
1–3, about frames carrying triangles, or about any other title. The oracle's
coverage-derived RGBA16 LSB is disclosed as a known second-order difference
and is not the cause of the disagreement reported here.
