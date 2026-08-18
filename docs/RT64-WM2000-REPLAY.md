# Replaying a real WM2000 packet through the Rust port

What happens when WWF WrestleMania 2000's **own captured RDP command words**
are fed to `fn64-render-wgpu`'s `WgpuBackend` through the production
`dispatch_dpc_submission` seam. Every number here comes from a run on this
machine.

Companion docs: [`RT64-WM2000-CENSUS.md`](RT64-WM2000-CENSUS.md),
[`RT64-WM2000-CYCLE-MODES.md`](RT64-WM2000-CYCLE-MODES.md),
[`RT64-PORT-CARD-BRIEF.md`](RT64-PORT-CARD-BRIEF.md) ("Measure, never
assert").

---

## 1. Headline: the packet is refused during PLANNING, at `LoadTLUT`

> **§1 and §1b supersede the withdrawn claim below.** A re-measurement on
> `58fcb964` found this section's original headline — "all 366 commands
> decode and plan successfully, then refuse at GPU submission" — to be
> **wrong for the packet this document describes**. What the corrected
> capture actually does is refuse *during planning*, at the seventh
> command's `LoadTLUT`, by name:
>
> ```
> plan_raw_dpc: render-wgpu/raw-dpc-plan backend error: raw-DPC plan probe
> decode failed: ... stream byte offset 0x2e0 wire opcode 0xf0:
> state-invalid command: LoadTLUT public macro requires a 16-bit
> destination tile descriptor
> ```
>
> `crates/fn64-render-wgpu/src/tmem/wire.rs:377`. Reproduced three
> consecutive times, identical each run. The coverage refusal quoted below
> is real but sits *behind* this one: it is reached only when the
> destination-descriptor check is bypassed (§1b).
>
> **The "366 of 366 decoded" figure is therefore withdrawn.** `LoadTLUT` is
> decoded and refused, so the admitted count is at most six commands, not
> 366. §5's burndown claim is corrected in place.
>
> **§1d supersedes this in turn.** The destination-descriptor check was
> measured against public libultra `gbi.h` and found **wrong**; it has been
> removed. Entry 0 now clears planning entirely and refuses at the coverage
> panic quoted immediately below — which restores the original headline's
> *refusal site*, though not its "366 of 366 admitted" wording (see §1d).
>
> **And §5 supersedes that.** The coverage panic §1d handed off to has since
> been narrowed, on a measured proof that WM2000's latched mode makes the
> unreadable `memory` term unobservable. **Both blockers are now closed and
> entry 0 executes and publishes with no bypass** — two independent fixes,
> one per blocker, neither sufficient alone. Entries 1–3 stop at a third,
> pre-existing frontier. See §5.

The withdrawn original claim, kept because §1a's correction is written
against it:

**All 366 commands of WM2000's frame 0 decode and plan successfully.** The
packet is then refused at GPU submission, by name:

```
submit_admitted_triangle received coverage_destination=Wrap with
image_read_enabled=true: this pipeline has no framebuffer-read mechanism to
supply a real memory coverage value (node 2, out of scope) -- must be
rejected before GPU submission, not silently substituted
```

`crates/fn64-render-wgpu/src/targets/triangle_pipeline.rs:282`.

Three facts locate that refusal precisely.

- **It is past decode, not in it.** `plan_raw_dpc_inner`
  (`crates/fn64-render-wgpu/src/production.rs:1938`) decodes the whole
  submission through `decode_raw_dpc` before `execute_raw_dpc` runs anything.
  Planning returning `Ok` is therefore the evidence that every one of the 366
  commands was admitted by the decoder — the opcode burndown for this packet
  is **366 of 366**, not a fraction.
- **It is reached through the texrect path, in a packet with zero
  triangles.** The census measured frame 0 as 60 `G_FILLRECT` + 60
  `G_TEXRECT` and no triangles at all
  ([census §5](RT64-WM2000-CENSUS.md)); the backtrace runs
  `execute_raw_dpc` → `draw_admitted_triangles` → `submit_triangles` →
  `fragment_coverage_params_bytes`. WM2000's texrects reach the triangle
  pipeline, and that is where they stop.
- **It is a `panic!`, not a returned `Err`.** Unlike the composition
  refusals, this frontier is spelled as a panic at the pipeline's submission
  boundary. It is deliberate and documented as such at the site ("node 2, a
  separate unresolved architectural decision"), but it is a different kind of
  refusal from the named `RawDpcDecodeError`/`TexrectExecutionError` variants,
  and a caller cannot match on it.

**Nothing is published.** The refused packet leaves its color image
byte-for-byte as it was poisoned before the replay. That is asserted, not
observed in passing — see §4.

### 1a. A correction to this card's own first measurement

The first replay attempted here reported a different refusal —
`LoadTLUT public macro requires a 16-bit destination tile descriptor`
(`crates/fn64-render-wgpu/src/tmem/wire.rs:379`) at command index 92 of 366.
**That number was wrong and is withdrawn.** It came from an incomplete
capture: the dump's first version hooked only the decoder's dispatch site,
which sees a command's leading `(w0, w1)` pair, so each 16-byte `G_TEXRECT`
lost its second pair — the S/T origin and the per-pixel gradients. Sixty
missing word pairs shifted every command after the first texrect, and the
"refusal" was the decoder correctly rejecting a corrupted stream.

The capture now records continuation words at the arm that decodes them
(`crates/fn64-render-reference/src/gbi/stream.rs:1334`), and the replay
checks that consecutive dumped rows are exactly 8 RDRAM bytes apart before
concatenating them. That contiguity check re-reads the old dump as a hard
error rather than a plausible packet, which is how the mistake was found.

The episode is recorded rather than quietly fixed because it is the exact
failure mode this card was told to guard against: a packet that *looks* real,
decodes far enough to produce a number, and is not the game's bytes.

### 1b. The re-measurement, and which fixture is the real one

The correction in §1 was found by replaying the capture again on a clean
`58fcb964` worktree. Two dumps exist on this machine and only one is the
packet this document describes:

| Dump | entry-0 rows | entry-0 wire bytes | Replay outcome |
|---|---|---|---|
| First (withdrawn, §1a) | 366 | 2,928 | Fails the test's own contiguity check |
| Corrected | 426 | **3,408** | Refused at `LoadTLUT` (§1) |

426 is the arithmetic §1a predicts: 366 commands with 60 variable-width
`G_TEXRECT`s contributing a second row each. **3,408 is §2's own stated wire-
byte figure**, and the corrected dump is byte-identical to the `entry0.bin`
blob captured alongside it — so the fixture that produces the `LoadTLUT`
refusal is the one this document was written about, not a third capture.
Two independent runs of the corrected capture are byte-identical, holding
§2's determinism bar.

**What the packet programs, hand-derived from its own words.** All seven
`LoadTLUT`s are identical in shape: tile 7, 16 entries, TMEM 256, preceded by
a `SetTile` with `siz=0` (4-bit) against a `SetTextureImage` with `siz=2`
(16-bit). That is the canonical libultra `gDPLoadTLUT_pal16` shape — a
16-entry palette for a 4-bit CI texture. §5's original text predicted exactly
this divergence and recorded that the check "did not fire on the correct
capture"; **that prediction was right about the shape and wrong about the
firing.** It fires on every one of the seven.

**What is behind the check, measured not reasoned.** Deleting the four-line
`descriptor.size()` guard as a throwaway probe (reverted; the worktree is
byte-clean against `58fcb964`) moves the refusal to
`triangle_pipeline.rs:280` — the coverage refusal §1 originally reported.
So the two refusals are ordered, not alternative, and the original headline
described the state reachable only with this guard bypassed.

**No claim is made that the guard is wrong**, and it was not changed. Two
pieces of evidence say it deserves a hardware-sourced answer rather than a
reading, and neither is sufficient alone:

- `descriptor.size()` is read at exactly **one** place in `tmem/wire.rs` —
  line 377, the refusal itself. `transfer_shape`'s `Tlut` arm (`:676-697`)
  sizes the transfer from `entries` and `image.size()`; the destination
  projection `project_tmem_transfer_word` (`tmem/types.rs:520`) reads only
  `descriptor.tmem()`. Nothing downstream consumes the field the check
  requires.
- This repository's *other* decoder already accepts the shape:
  `decode_ci4_pal16_load_uses_palette_local_indices`
  (`crates/fn64-render-reference/src/gbi/tests/group5.rs:67`) loads a
  16-entry pal16 TLUT for a `G_IM_SIZ_4B` texture.

The refusal has **no test of its own** (`grep` for its message string returns
only the site). That is a gap this card records and did not fill.

### 1d. Resolved: the destination-descriptor check was wrong, and is removed

§1b left the question open pending external authority. That authority was
obtained, and it refutes the check.

**Evidence — public libultra `gbi.h`, the macro bodies themselves.** The
guard's own comment claimed "the macro always programs a 16-bit-per-entry
palette image". That is true of the *source image* and false of the
*destination tile descriptor*. `gDPLoadTLUT_pal16(pkt, pal, dram)` expands to:

```c
gDPSetTextureImage(pkt, G_IM_FMT_RGBA, G_IM_SIZ_16b, 1, dram);
gDPTileSync(pkt);
gDPSetTile(pkt, 0, 0, 0, (256+(((pal)&0xf)*16)),
        G_TX_LOADTILE, 0 , 0, 0, 0, 0, 0, 0);
gDPLoadSync(pkt);
gDPLoadTLUTCmd(pkt, G_TX_LOADTILE, 15);
gDPPipeSync(pkt)
```

`gDPSetTile`'s parameter order is `(pkt, fmt, siz, line, tmem, tile, ...)`
(`sm64-decomp/include/PR/gbi.h:3401`), so the second `0` is **`siz`**, and
`G_IM_SIZ_4b == 0` (`:410`). The canonical destination `siz` for a TLUT load
is therefore 4-bit, **never** 16-bit — the exact value the check refused.
`gDPLoadTLUT_pal256` (`:4283`) and the generic `gDPLoadTLUT` (`:4331`)
program the same `siz == 0`.

The macro is byte-identical in four independent SDK copies on this machine,
which is why it is read as the SDK's text rather than one project's
transcription: `sm64-decomp/include/PR/gbi.h:4229`,
`mm-decomp/include/PR/gbi.h:4655`, `kirby64-decomp/include/PR/gbi.h:4239`,
`oot-decomp/include/ultra64/gbi.h:4657`. Read under AGENTS.md's
"public libultra manuals" allowance — header text, not any project's code.

**Why the field is unconstrained rather than newly constrained to 4-bit.**
The load tile describes a TMEM region for a quadricated palette write, not
the palette's pixel format, and no code consumes the field for this kind:
`transfer_shape`'s `Tlut` arm sizes from `entries` and `image.size()`, and
`project_tmem_transfer_word`'s `Tlut` arm reads only `descriptor.tmem()` and
`line_words()`. After the change `descriptor.size()` has **zero** readers in
`tmem/wire.rs`. Constraining it to `Bits4` would refuse a shape the hardware
has no reason to reject and that nothing in this module can mis-size; that
mutant is tested and rejected below.

**The sibling check at `:372` is correct and is retained.** The macro really
does always emit `G_IM_SIZ_16b` for the *source* `SetTextureImage`, and the
transfer shape is sized from it.

**How the bug survived review.** The `set_tile` test helper
(`raw_dpc/mod.rs:2225`) hardcodes `2 << 19` — `siz == 2` — so every
pre-existing `LoadTLUT` fixture shared the guard's mistaken assumption and
none could reach the canonical shape. The refusal had no test of its own.

**The test the refusal lacked**, added at `raw_dpc/mod.rs`:
`load_tlut_accepts_the_canonical_macro_four_bit_destination_descriptor`
builds the `SetTile` word directly rather than through the helper, asserts
all four destination sizes decode with an identical transfer shape (16
entries, 32 source bytes), and asserts the source check still refuses
non-16-bit `SetTextureImage`. Three mutants, three kills: restoring the
original `Bits16` guard fails it; deleting the `:372` source check fails it;
narrowing the destination to `Bits4`-only fails it.

### 1c. The GPU triangle draw is not redundant for texrects

This card was dispatched to test whether WM2000's texrects could stop being
routed through the triangle pipeline, on the observation that
`targets/texrect.rs` is a self-contained CPU executor. **The observation is
correct and the conclusion drawn from it is not.** The routing change was
not made.

True: a texrect's GPU-rasterized *pixels* are guest-unobservable.
`draw_admitted_triangles` stores its readback only into
`triangle_draw_output` (`production.rs:541`), whose sole reader is the
`last_triangle_draw()` diagnostic accessor (`:331`) — called from `#[cfg(test)]`
code only, with no non-test reader anywhere in the workspace. `present()`
(`:1632`) refuses to treat it as a framebuffer by name. Every guest-visible
pixel comes from the CPU path, published through `pending_fill_publication`
→ `staged_guest_render_target_writes` (`:1799`, `:1804`), and
`targets/texrect.rs` contains zero references to the triangle pipeline.

False, and the reason the change must not be made: the draw's **`Result` is a
gate on that publication.** The `?` at `production.rs:1779` returns before
`self.pending_fill_publication = pending;` at `:1799`, so a failed draw
discards pixels the CPU executor had already computed. This is deliberate,
documented at `:1785-1788`, and pinned by a source-shape test
(`a_failed_triangle_draw_leaves_no_redeemable_fill_token`, `:7049`, asserting
the textual ordering at `:7112-7125`). The `TmemSampleFailed` check
(`:533-540`) reads the GPU readback *before* it is stored, and is a real
second sampler over the same projection that the CPU path does not duplicate
— `raw_dpc_session_integration.rs:4030-4034` records a measured texrect where
that gate is what kept guest RDRAM at its poison.

Removing the call for texrects would therefore make packets *succeed* that
currently fail — an adapterless host's `TriangleDrawBeforeCreate` (`:392`)
would stop blocking a submission — which is a weakened refusal, not a
routing change. The defensible claim is narrower than the card's premise:
**the GPU draw contributes no guest-visible pixels for a texrect and is
retained as a validation gate.** Reclassifying it as vestigial is
unsupported.

---

## 2. How the capture is produced

**The tooling**, committed and re-runnable as a burndown instrument:

- `crates/fn64-render-reference/src/gbi/census.rs`, module `packet` — an
  env-gated dump of the raw `(w0, w1)` pairs `decode_stream_impl` dispatched
  on, hooked at the same site the opcode histogram counts from
  (`gbi/stream.rs:269`) and from the same bindings, so a dumped word pair and
  a census row cannot disagree about what was decoded. Variable-width
  `G_TEXRECT` continuations are recorded at their own decode site
  (`gbi/stream.rs:1334`), which is what makes the dumped rows reconstruct the
  wire byte stream with no gaps.
- Bounded by construction: `FN64_GBI_PACKET_DUMP_ENTRIES` names the decode
  entries to keep, so the row vector does not grow with the run. This is why
  it needs no separate "this grows without bound" knob of the kind
  `FN64_GBI_TEXRECT_CENSUS` carries.
- `examples/wm2000-census/` — the same headless harness the two censuses use,
  carrying zero game content, flushing on the same incremental cadence and
  for the same reason: the run ends in a non-unwinding abort.

**The exact command.**

```sh
cd examples/wm2000-census
RECOMPILED_DIR="$HOME/Code/aki-recomp/games/NWXE/RecompiledFuncs" \
RECOMP_H_DIR="$HOME/Code/wm2000-run/recomp-h-clean" \
ROM="$HOME/Code/aki-recomp/games/NWXE/wm2000.z64" \
  cargo build --release

FN64_GBI_CENSUS=1 \
FN64_GBI_CENSUS_PER_TASK=1 \
FN64_GBI_CENSUS_OUT=<scratch>/census.tsv \
FN64_GBI_PACKET_DUMP=1 \
FN64_GBI_PACKET_DUMP_ENTRIES=0,1,2,3 \
FN64_GBI_PACKET_DUMP_OUT=<scratch>/packet.tsv \
WM2000_MAX_STEPS=20000000 \
ROM="$HOME/Code/aki-recomp/games/NWXE/wm2000.z64" \
  ./target/release/wm2000-census
```

**The ROM.** `/Users/jer/Code/aki-recomp/games/NWXE/wm2000.z64`, SHA-1
verified with `shasum -a 1` against the `rom_sha1` at
`aki-recomp/games/NWXE/profile.toml:14` — the same ROM and the same check the
two censuses used.

**Determinism.** Two full runs produced byte-identical dumps: `diff` clean
and identical under `shasum -a 256`. The census files matched too. This holds
the bar both prior censuses set.

**What was captured.** Decode entries 0–3, all triangle-free:

| Entry | Commands | Distinct opcodes | Wire bytes | Triangles |
|---|---|---|---|---|
| 0 | 366 | 19 | 3,408 | 0 |
| 1 | 592 | 20 | 5,416 | 0 |
| 2 | 592 | 20 | 5,416 | 0 |
| 3 | 592 | 20 | 5,416 | 0 |

Entry 0's 366 commands and 19 distinct opcodes match
[census §5](RT64-WM2000-CENSUS.md) exactly, as do its 60 `G_FILLRECT` and 60
`G_TEXRECT`. Those counts were produced by a different instrument (the opcode
histogram) than the dump, so their agreement is two independent counters over
one packet.

**What entry 0 actually programs**, read off its own words: an RGBA16 color
image 480 wide at RDRAM `0x0038f800`, a 480x240 scissor, fill colour
`0x00010001`, 60 fill rectangles of 16 rows each, seven `LoadTLUT`s into
TMEM 256, and a latched other-mode high word of `0x00acef` — whose
`G_MDSFT_CYCLETYPE` field is zero, i.e. one-cycle, matching
[cycle-modes §1](RT64-WM2000-CYCLE-MODES.md)'s `0x0000acef`.

---

## 3. How the replay is run

The packet is **not committed**. `README.md`'s "no game content ships in this
repo" rule covers recompiled-game output, and a game's own RDP command words
are exactly that. The test reads the dump from an out-of-tree path:

```sh
FN64_WM2000_PACKET_TSV=<scratch>/packet.tsv \
FN64_WM2000_PACKET_ENTRY=0 \
  cargo nextest run -p fn64-abi --offline \
  -E 'test(a_real_wm2000_packet_replayed_through_wgpu_backend)' --no-capture
```

With `FN64_WM2000_PACKET_TSV` unset the test prints what it did not run and
returns. Set-but-unreadable, or set to a malformed dump, is a hard error by
name — a silent pass would let an operator read green as evidence the replay
ran.

The test drives the real `crate::task_dispatch::dispatch_dpc_submission`
producer entry against a real 8 MiB RDRAM allocation with a real
`WgpuBackend` + `RawDpcAbiSession` registered, exactly as this file's
synthetic end-to-end tests do. The color-target extent is read from the
packet's own `SetColorImage` and `SetScissor` rather than hardcoded, so the
harness cannot be the thing that caps the replay.

All four captured entries were replayed. **All four produce the identical
refusal**, at the same site. This is one systematic frontier, not a scatter.
That conclusion survives §1's correction, re-measured: on the corrected
capture all four entries refuse with `LoadTLUT public macro requires a
16-bit destination tile descriptor` at wire opcode `0xf0`, not with the
coverage refusal originally recorded here.

---

## 4. What is asserted, and what is not

The test does **not** pin "executes" or "is refused with X". That frontier is
expected to move, and a test that ratchets it would have to be edited by the
next slice. What it pins instead is the packet's own identity — so the
fixture cannot quietly become a synthetic stand-in — and the one behavioural
property a refusal must have.

**Asserted:**

- The dump's rows reconstruct a contiguous wire stream (consecutive rows
  exactly 8 RDRAM bytes apart). This is what makes concatenating the pairs
  legitimate, and it is what caught §1a's incomplete capture.
- Entry 0 is 366 commands, 19 distinct opcodes, 60 `G_FILLRECT`, 60
  `G_TEXRECT`, zero triangles — the census's numbers, checked against the
  fixture.
- The latched cycle type is one-cycle, read off the packet's own final
  `SetOtherMode` word rather than transcribed from the cycle-modes probe.
- **A refused packet publishes nothing**: the color image is byte-for-byte
  the poison written before the replay. A partial write would mean some
  commands published while the rest were refused, which is the "plausible
  pixels without a proven draw" outcome this line of work must not produce.
- If a packet ever does execute with no refusal, it must have changed its own
  color image. That arm is written and unreached today.
- A refusal must be a named frontier, not an index-out-of-bounds, an overflow,
  or an `Option::unwrap` on `None`. Those would be defects found by a real
  packet, which is a different finding and must not be reported as burndown.

**Not asserted, and not provable here:**

- **No pixel values.** Nothing renders, so there is nothing to check the
  fill's even/odd RGBA16 column rule or the combiner's output against. The
  hand-derived-extent and combiner-output assertions this card's brief
  contemplated are unreachable while the refusal stands; they are
  written for the synthetic fixtures elsewhere in the same file and would
  transfer directly once a real packet gets past it. (Per §1 the standing
  refusal is `LoadTLUT`, not coverage; the assertions are unreachable behind
  either.)
- **No frame.** See §5.
- **The target extent derivation is not pinned.** Mutating the
  `SetColorImage` width — dropping the wire field's `+1`, and even forcing
  the width to 1 — does not change the outcome, because the refusal
  fires before any fill is sized. The derivation is kept because it is
  correct and because it stops the harness becoming the frontier the moment
  that refusal moves, but it is a proven-equivalent mutant today and is
  disclosed as one rather than defended. §1's correction moves the refusal
  earlier still (to planning), which widens this equivalence rather than
  narrowing it.

---

## 5. Verdict: how close is this to "WM2000 renders through the Rust port"?

**Not close, and closer than the last measurement could see.** Both halves
are real.

What is proven that was not before: WM2000's actual bytes reach
`WgpuBackend` through the production seam, and the refusals they meet are
named ones at documented frontiers rather than defects in the executor's
arithmetic. The census's §7 items 1 and 2 (`SyncPipe`/`SyncTile`/`0x1f`) have
landed, and the composition refusals that §4a measured at 100% of frames no
longer fire on this packet.

**The "366 of 366 admitted" figure is withdrawn** (§1). The packet is refused
at its seventh command, so the observed admitted count is at most six. The
census's §8 caveat — "the ADMITTED column is classification against that
decoder's source, not an observed acceptance" — therefore still stands
unconverted for the remaining 360.

~~What is not proven, and is the whole distance remaining: **not one pixel of
WM2000 has been rendered.** No fill reached its color image, no texel was
sampled, nothing was published.~~ **Superseded — see "Both blockers are now
closed" below.** Entry 0 executes end-to-end with no bypass and publishes
230,400 bytes into its own color image. It took *two independent fixes*, one
per blocker, and neither alone was sufficient.

**The `LoadTLUT` blocker is closed** (§1d). It was measured against public
libultra `gbi.h`, found wrong, and removed; the canonical
`gDPLoadTLUT_pal16` destination `siz` is `G_IM_SIZ_4b == 0`, never 16-bit.
Entry 0's 366 commands now all decode and plan, so **the "366 of 366
decoded" figure is restored** — but as an observed acceptance this time,
not the inference §1 withdrew. Measured, not quoted: workspace 8279→8280
passed / 13 skipped (the one new test), debug and release profile, dead-code
count unchanged at 1217 (re-measured both sides, not carried from a brief).

**A new frontier appeared behind it, in entries 1–3 only.** Those entries now
refuse at `assert_eq!(source_accesses.len(), 1,
"v11's admitted TMEM source plan is exactly one journal access wide")`
(`raw_dpc/production_adapter.rs:1224-1228`; the panic reports `:1224`, the
message text sits at `:1227`). This is a documented v11 adapter scope limit —
the decoder's own `source_accesses` admits up to `MAX_RESOURCE_ACCESSES`
ranges — not a decoder defect. It was previously masked by the `LoadTLUT`
refusal and is recorded here as newly visible, not newly introduced. It is an
`assert_eq!` rather than a named refusal, so like the coverage panic a caller
cannot match on it.

**Independently confirmed after the coverage narrowing landed**, with no
bypass: entries 1, 2 and 3 all stop here, each reporting left `49` right `1`.
The coverage fix did not move this frontier and does not address it. Entry 0
alone reaches publication.

### The second blocker: coverage, narrowed on non-observability

The section above correctly reported the coverage refusal as the standing
blocker for entry 0, and correctly said its size assessment "stands: the
pipeline has no framebuffer-read mechanism". **That premise was right about
the mechanism and wrong about what WM2000 needs from it.** The correction is
measured, and it does *not* retract §1d — the two blockers were independent
and each needed its own fix.

`CoverageDestination::Clamp`/`Wrap` with `image_read_enabled`
(`triangle_pipeline.rs`) now refuses only when the unknown `memory` value can
reach a shader output: `alpha_coverage_select || !force_blend`.

**What the packet actually latches**, read off its own words rather than
assumed. All 60 of entry 0's texrects latch other-mode low `0x005041c8` —
`cvg_dst=Wrap`, `IM_RD`, `AA_EN`, `CLR_ON_CVG`, `FORCE_BL`, with
**`CVG_X_ALPHA` and `ALPHA_CVG_SEL` both clear**. The 60 fillrects latch
`0x00000000` (no image read at all).

**Why those two clear bits decide it.** `coverage_fragment_fn`'s result
reaches `FragmentOutput` by exactly two routes
(`shaders/triangle_pipeline_fragment.wgsl`): `output.color.a`, guarded by
`alpha_coverage_select`; and `blend_enabled`, which is
`force_blend || (antialias_enabled && !wraps)`, where `wraps` is the
`memory`-dependent term. With `ALPHA_CVG_SEL` clear the first route is dead;
with `FORCE_BL` set the second short-circuits to `true` for every `memory`
value. `wraps` is otherwise unexported — this pipeline has no
`clear_on_coverage` discard, the CPU reference's consumer
(`raster/draw.rs`'s `set_blended`) having no counterpart here.

**The admission rests on non-observability, not on vacuity.** `memory` is
emphatically *not* a no-op in the accumulation: `destination` genuinely varies
across `memory` in `0..=8` under this very mode, and a test asserts that it
does, precisely so this cannot be misread as "the math doesn't matter". What
is proven is narrower and sufficient — that under this bit combination no
output the draw produces is a function of the term the pipeline cannot supply.

**Nothing is read and nothing is substituted.** `memory_count` remains the
shader's `0u` literal; the serialized `coverage_destination` word stays the
mode's own encoding (Clamp=0/Wrap=1), never a substituted `Full`(2). `Save`
still refuses unconditionally, since `destination = memory` has no
`memory`-independent case at all.

**Two refuted hypotheses, recorded because each killed a plausible plan.**

- *"The CPU executor already knows the destination coverage."* False. The CPU
  texrect executor implements **no coverage at all** — its own module doc says
  so (`targets/texrect.rs`, "No blending, alpha compare, dither, or
  coverage"). Only the unwired `fn64-render-reference` rasterizer keeps a
  coverage buffer.
- *"A framebuffer-read mechanism must be built from scratch."* Half false, and
  the useful half is the correction: a framebuffer **color** read already
  exists and works — `framebuffer_color_snapshot` (binding 9), a
  `copy_texture_to_buffer` snapshot with per-draw run splitting
  (`split_fixture_runs`). The real gap is that nothing *writes* coverage:
  `TriangleDrawOutput` carries color/depth/status only, and
  `production.rs`'s `BlendRequiresFramebuffer` doc states "no coverage-count
  GPU write exists anywhere in this crate". So the remaining work is a
  specific missing attachment, not an absent read path.

### Both blockers are now closed, and entry 0 publishes

**Measured with no bypass of any kind**, on the tip carrying both fixes: entry
0's 366 commands **execute end-to-end through `WgpuBackend` with no refusal**,
and the replay's previously-unreached arm — "a packet that executed with no
refusal must have published something into its own color image" — fires and
passes.

What landed in guest RDRAM, hand-reconciled against the packet's own words:

| Quantity | Measured | Derived from the packet |
|---|---|---|
| Color image | 480x240 RGBA16, 230,400 bytes | `SetColorImage` + `SetScissor` |
| Bytes differing from poison | 229,496 (99.6%) | — |
| Distinct pixel values | 2 | — |
| `0xffff` | 114,481 px (99.4%) | the 60 texrects' output |
| `0x0001` | 719 px (0.6%) | `SetFillColor 0x00010001`, two RGBA16 `0x0001` |

`114,481 + 719 = 115,200 = 480 x 240` exactly, so every pixel is accounted
for. The 60 fillrects cover exactly 240 rows (hand-summed from their own
`(yh - yl + 1)` spans) and the 60 texrects sum to ~7,155 px of area, which is
why the fill colour survives only where no texrect landed.

**This is the first time WM2000's own bytes have published pixels through the
Rust port.** It is not a rendered frame: two distinct values is a plausible
title-screen-shaped result for a packet whose combiner reads a 4-bit CI
texture, but **no pixel is validated against hardware or against the CPU
reference**, and a uniform `0xffff` majority is equally consistent with a
texel decode that saturates. Treat it as "the pipeline is now end-to-end
reachable", not as "the image is correct".

Two smaller things remain on the path:

- The coverage refusal is a `panic!` rather than a named error variant, so it
  cannot be matched or counted by a caller the way the decode refusals can.
  The narrowed refusal keeps that shape.
- ~~The `LoadTLUT` destination check has no test~~ — closed, §1d.

**Nonclaims.** No frame is rendered. No pixel is asserted. No routing was
changed — §1c explains why the change that card was dispatched to make would
have weakened a refusal. One refusal **was** removed (§1d), on external
header evidence and with three mutation kills, not on inference; the
neighbouring source-image check was retained and is pinned by the same test.
Removing it admits `LoadTLUT` to *planning* only — it makes no claim that the
TLUT is correctly written to TMEM, since nothing executes past the coverage
panic. The v11 access-count frontier in entries 1–3 is reported as newly
visible, not diagnosed or fixed, and it is unchanged by the coverage
narrowing (re-confirmed on all three entries with no bypass).

**Nonclaims for the coverage narrowing specifically.** No framebuffer-read
mechanism is implemented and no coverage value is substituted; the refusal was
*narrowed*, not deleted. `Save`, and `Clamp`/`Wrap` with `ALPHA_CVG_SEL` set
or `FORCE_BL` clear, still refuse loudly, and a real triangle with
`image_read_enabled` under those bits is refused exactly as before — four
tests pin that boundary. **The narrowing is WM2000-shaped: a title that sets
`ALPHA_CVG_SEL` or clears `FORCE_BL` still hits the same wall**, which needs
the missing coverage attachment described above, not a wider constant. The
`0xffff`/`0x0001` pixel values are reported as measured bytes, not as
validated output. Everything measured is entry 0 of one window of one title and
says nothing about gameplay, which the `0x1CC` MMIO abort still bounds this
capture short of. The four captured entries are all triangle-free early
frames; the 152 of 218 frames the census measured as carrying triangles were
not replayed.
