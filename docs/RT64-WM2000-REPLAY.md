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

## 1. Headline: the packet decodes completely, and is refused during execution

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
  contemplated are unreachable while the coverage refusal stands; they are
  written for the synthetic fixtures elsewhere in the same file and would
  transfer directly once a real packet gets past it.
- **No frame.** See §5.
- **The target extent derivation is not pinned.** Mutating the
  `SetColorImage` width — dropping the wire field's `+1`, and even forcing
  the width to 1 — does not change the outcome, because the coverage refusal
  fires before any fill is sized. The derivation is kept because it is
  correct and because it stops the harness becoming the frontier the moment
  that refusal moves, but it is a proven-equivalent mutant today and is
  disclosed as one rather than defended.

---

## 5. Verdict: how close is this to "WM2000 renders through the Rust port"?

**Not close, and closer than the last measurement could see.** Both halves
are real.

What is now proven that was not before: WM2000's actual bytes reach
`WgpuBackend` and its decoder admits **all of them**. The opcode-admission
question the census could only classify against source — its own §8 says so
plainly: "the ADMITTED column is classification against that decoder's
source, not an observed acceptance" — is now an observation. For this packet
it is 366 of 366. The census's §7 items 1 and 2 (`SyncPipe`/`SyncTile`/`0x1f`)
have landed, and the composition refusals that §4a measured at 100% of frames
no longer fire on it.

What is not proven, and is the whole distance remaining: **not one pixel of
WM2000 has been rendered.** No fill reached its color image, no texel was
sampled, nothing was published. The packet gets through decode and stops at
the first thing that tries to draw.

The next blocker, with its size:

**`CoverageDestination::Clamp`/`Wrap` with `image_read_enabled` set.** Every
WM2000 texrect in this packet latches it. The refusal is deliberate and its
site documents why: the pipeline has no framebuffer-read mechanism, so it
cannot supply a real `memory: Coverage` value, and substituting one silently
is the failure `AGENTS.md` forbids. **Size: architectural, not a match arm.**
The site names it "node 2, a separate unresolved architectural decision" —
it needs a framebuffer-read path, which is a mechanism this backend does not
have, not a constant to widen. It is not one line and this card did not
attempt it.

Two smaller things are visible behind it and are not claims about what
happens after, only about what is on the path:

- The refusal is a `panic!` rather than a named error variant, so it cannot
  be matched or counted by a caller the way the decode refusals can.
- WM2000's `LoadTLUT` destination tiles are programmed `size=0` (4-bit) with
  a 16-bit source image, against a check that requires a 16-bit destination
  descriptor (`tmem/wire.rs:377`). That check did not fire on the correct
  capture, so **no claim is made that it is wrong** — but it is a real
  divergence between what this title programs and what the check expects, and
  it is the kind of thing that will need hardware evidence rather than a
  reading of the libultra macro if it ever does fire.

**Nonclaims.** No frame is rendered. No pixel is asserted. The 366-of-366
decode figure is for entry 0 of one window of one title and says nothing
about gameplay, which the `0x1CC` MMIO abort still bounds this capture short
of. No refusal was weakened and none was fixed. The four captured entries are
all triangle-free early frames; the 152 of 218 frames the census measured as
carrying triangles were not replayed.
