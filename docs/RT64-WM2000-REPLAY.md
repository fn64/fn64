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

What is not proven, and is the whole distance remaining: **not one pixel of
WM2000 has been rendered.** No fill reached its color image, no texel was
sampled, nothing was published.

The next blocker, with its size — **this is a different, smaller blocker than
this section originally named**:

**`LoadTLUT` with a 4-bit destination tile descriptor** (`tmem/wire.rs:377`).
All seven of the packet's `LoadTLUT`s are the canonical `gDPLoadTLUT_pal16`
shape (§1b). **Size: one check, pending evidence — not architectural.** The
field it tests is read nowhere else in the file, and this repo's own
reference decoder already accepts the shape, so the likely resolution is that
the check is over-strict. But "likely" is not measured: it needs the libultra
manual section or hardware evidence, plus the test the refusal currently
lacks. Deciding it by reading the code that fails would be the reasoning this
project forbids.

**Behind it, unchanged and still architectural:**
`CoverageDestination::Clamp`/`Wrap` with `image_read_enabled` set, at
`triangle_pipeline.rs:280`. Reaching it required bypassing the `LoadTLUT`
guard (§1b), so it is the *second* blocker, not the first. Its size
assessment stands: the pipeline has no framebuffer-read mechanism, its site
names it "node 2, a separate unresolved architectural decision", and it needs
a mechanism rather than a widened constant. Note that §1c's finding does not
shrink it — the texrect route to that panic cannot simply be removed, because
the draw is a publication gate.

Two smaller things remain on the path:

- The coverage refusal is a `panic!` rather than a named error variant, so it
  cannot be matched or counted by a caller the way the decode refusals can.
- The `LoadTLUT` destination check has no test (§1b).

**Nonclaims.** No frame is rendered. No pixel is asserted. No refusal was
weakened, none was fixed, and no routing was changed — §1c explains why the
change this card was dispatched to make would have weakened one. The
`descriptor.size()` probe in §1b was reverted; the worktree is byte-clean
against `58fcb964`. Whether the `LoadTLUT` check is correct is **not**
decided here. Everything measured is entry 0 of one window of one title and
says nothing about gameplay, which the `0x1CC` MMIO abort still bounds this
capture short of. The four captured entries are all triangle-free early
frames; the 152 of 218 frames the census measured as carrying triangles were
not replayed.
