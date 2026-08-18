# WM2000's command census: what the game actually asks for

A counted histogram of every graphics command WWF WrestleMania 2000 (NWXE)
issues over its boot-through-attract window, produced by running the real
recompiled game, and the resulting ADMITTED/REJECTED split against
`fn64-render-wgpu`'s `WgpuBackend`.

This closes the measurement [`RT64-WM2000-GAP.md`](RT64-WM2000-GAP.md) §2
could not take and §5 named as the first thing to build. Every number here
comes from a run on this machine; nothing is estimated. Companion docs:
[`RT64-WM2000-GAP.md`](RT64-WM2000-GAP.md),
[`RENDER-WGPU-PORT-PLAN.md`](RENDER-WGPU-PORT-PLAN.md),
[`RT64-PORT-CARD-BRIEF.md`](RT64-PORT-CARD-BRIEF.md) ("Measure, never
assert").

---

> **CORRECTION (2026-08-18, commit `8fbef998`) — §1's inference is REFUTED by
> measurement. Both counts below are correct; the conclusion drawn from them is
> not.**
>
> **WM2000 does hand the system a GBI display list.** Dumped from the real ROM
> at `dispatch_gfx_task_chunk`: `task_type=1` (M_GFXTASK), `data_ptr=0x38ce30`,
> `data_size=1008`, a well-formed **F3DEX2 list of 126 commands terminating in
> `G_ENDDL`** — 60 `G_FILLRECT`, 13 `G_MOVEWORD`, 11 `G_DL`, 10 each
> `G_MOVEMEM`/`G_SETPRIMCOLOR`/`G_SETENVCOLOR`, plus `SETOTHERMODE_H/L`,
> `GEOMETRYMODE`, `SETCIMG`, `SETFILLCOLOR`, sync ops and `G_TEXTURE`.
>
> **Why `gbi_lane_commands` reads 0 anyway:** WM2000's live IMEM hashes to
> `c50d2949c23baae24e706e8e1a5abf2dd315d00aff4cfdd567a03fe81807d1be`, which is
> in no `GeometryUcodeCatalog`. `require_text` returns `RequiresLle`,
> `process_task` returns `NeedsLle`, and **the GBI decoder is never entered.**
> `fn64-abi` then runs the microcode on the RSP interpreter, whose RDP writes
> return through XBUS as the 142,606 raw-DPC commands counted below. This also
> resolves §8's open UNKNOWN about 106 tasks not reaching a top-level decode
> entry.
>
> **A dumping trap worth recording:** the first dump used raw `from_be_bytes`
> over the swizzled physical slice and looked like garbage. Reading through
> `RdramView`'s logical lane mapping is essential.
>
> Consequence: "no display-list front end is needed for this title" was the
> right *action* for the wrong *reason*. Both `ReferenceBackend` and
> `Rt64Backend` gate on the ucode and defer to LLE; `WgpuBackend` now does the
> same rather than erroring.

## 1. Headline: WM2000 is 100% raw-DPC, and 84% of it is already admitted

Three findings, each of which corrects the standing scoping picture.

**WM2000 issues zero GBI display-list commands.** The census counts both
lanes the decoder dispatches on, and the RSP-side GBI lane is empty:
`gbi_lane_commands 0` against `rdp_lane_commands 142606`. Every graphics
command the game issues arrives as a raw RDP command through the XBUS
submission path. Corroborated independently in-tree: the comment at
`crates/fn64-render-reference/src/gbi/entries.rs:277-278` already records
"Measured live with `sample` on WM2000 (100% raw-RDP/XBUS submission, ~18,838
tasks/route)" from unrelated profiling work.

This reverses the gap doc's §1 verdict about which seam matters. That section
concluded the raw-DPC IR seam "is not on WM2000's path today"; the reasoning
was correct about the *shell's* routing — nothing registers a
`RawDpcAbiSession` — but wrong about the *game*. WM2000 does not hand the
system a GBI display list to be walked. It hands it RDP commands. A
display-list front end of the size
`crates/fn64-render-reference/src/gbi/` represents is not what this title
needs; what it needs is for the existing raw-DPC executor to be reachable
from the shell.

**84.0% of issued commands are already in the decoder's admitted set.**
119,779 of 142,606. Exactly three opcodes are rejected, and two of those are
no-ops the reference backend already treats as a no-op group.

**Every frame uses 19-21 distinct opcodes.** There is no cheap early frame.
The smallest single frame in the window (the very first, 366 commands) still
issues 19 distinct opcodes. §5 works through what this means for milestone
selection, and it is the finding that most changes the plan.

---

## 2. How this was produced

**The tooling.** Two committed pieces, both re-runnable:

- `crates/fn64-render-reference/src/gbi/census.rs` — an env-gated counter
  hooked into the decoder's command dispatch at
  `crates/fn64-render-reference/src/gbi/stream.rs:265`, the single point
  every command in both lanes passes through. Off unless `FN64_GBI_CENSUS` is
  set; when off, `note` loads one relaxed atomic and returns. Opcode names
  come from the crate's own `state::opcode_name`, the table the decoder's
  unsupported-command panic already prints from, so a census row and a decode
  failure name the same command identically.
- `examples/wm2000-census/` — a headless harness that boots the real
  recompiled game against `ReferenceBackend` and writes the histogram.
  Contains zero game content: `RECOMPILED_DIR`, `RECOMP_H_DIR`, and `ROM` all
  point out of tree, per `README.md`'s "no game content ships in this repo"
  rule. Derived from the out-of-tree `wm2000-boot` harness the gap doc found
  291 commits stale; the boot sequence is unchanged from it, because a census
  of a different boot is a census of a different game.

**The exact command.**

```sh
cd examples/wm2000-census
RECOMPILED_DIR="$HOME/Code/aki-recomp/games/NWXE/RecompiledFuncs" \
RECOMP_H_DIR="$HOME/Code/wm2000-run/recomp-h-clean" \
ROM="$HOME/Code/aki-recomp/games/NWXE/wm2000.z64" \
  cargo build --release

FN64_GBI_CENSUS=1 \
FN64_GBI_CENSUS_PER_TASK=1 \
FN64_GBI_CENSUS_OUT=/tmp/wm2000-census-out/census.tsv \
WM2000_MAX_STEPS=20000000 \
ROM="$HOME/Code/aki-recomp/games/NWXE/wm2000.z64" \
  ./target/release/wm2000-census
```

`FN64_GBI_CENSUS_PER_TASK` additionally records per-decode-entry deltas,
which is what §5's per-frame analysis is computed from. It is a separate knob
because the snapshot vector grows without bound.

**The ROM.** `/Users/jer/Code/aki-recomp/games/NWXE/wm2000.z64`, SHA-1
matching the `rom_sha1` recorded at `aki-recomp/games/NWXE/profile.toml:14`
(verified with `shasum -a 1`; this doc does not restate the digest, since no
test here gates it).

**The window: boot through 383 VI fields, 325 gfx tasks, 219 decoded
display-list entries.** About 6.4 seconds of NTSC virtual time — the boot
sequence, logo/title screens, and the attract loop. The run ends at a
reproducible guest-side abort, not at a step budget:

```
generated-C direct-device read at 0x00000000000001CC used unsupported mapped
address with width 4; only zero- or sign-extended KSEG0/KSEG1 are modeled
```

(`crates/fn64-abi/src/lib.rs:540`.) That is an unmodelled MMIO access, a
separate defect from anything this census measures, and it is what bounds the
window. **This is not the `MAX_TASK_STEPS` abort the gap doc flagged as the
open question** — current HEAD clears that bound. Two blockers were found and
one was fixed to get here; see §6.

**Reproducibility.** Two full runs produced byte-identical census files. The
counts below are deterministic, not a sample.

**Why 219 decode entries against 325 gfx tasks.** 106 tasks do not reach a
top-level decode entry. UNKNOWN why; the census counts what the decoder
dispatched, and does not speculate about tasks it never saw.

---

## 3. The histogram

Full census, cumulative over the window. `cmd6` is the 6-bit RDP command id
(`byte & 0x3f`), which is what `WgpuBackend`'s decoder matches on
(`crates/fn64-render-wgpu/src/raw_dpc/mod.rs:1066`).

| Opcode | Byte | cmd6 | Count | % | Status |
|---|---|---|---|---|---|
| `G_RDPSETOTHERMODE` | `0xef` | `0x2f` | 23,639 | 16.6% | ADMITTED |
| `G_RDPPIPESYNC` | `0xe7` | `0x27` | 20,800 | 14.6% | **REJECTED** |
| `G_SETTILE` | `0xf5` | `0x35` | 19,454 | 13.6% | ADMITTED |
| `G_FILLRECT` | `0xf6` | `0x36` | 13,140 | 9.2% | ADMITTED |
| `G_RDPLOADSYNC` | `0xe6` | `0x26` | 10,631 | 7.5% | ADMITTED |
| `G_SETTIMG` | `0xfd` | `0x3d` | 10,631 | 7.5% | ADMITTED |
| `RDP_TRI_SHADE_TEX` | `0x0e` | `0x0e` | 10,380 | 7.3% | ADMITTED |
| `G_SETTILESIZE` | `0xf2` | `0x32` | 8,823 | 6.2% | ADMITTED |
| `G_LOADTILE` | `0xf4` | `0x34` | 7,290 | 5.1% | ADMITTED |
| `G_SETENVCOLOR` | `0xfb` | `0x3b` | 3,397 | 2.4% | ADMITTED |
| `G_SETPRIMCOLOR` | `0xfa` | `0x3a` | 2,691 | 1.9% | ADMITTED |
| `G_TEXRECT` | `0xe4` | `0x24` | 2,520 | 1.8% | ADMITTED |
| `G_SETCOMBINE` | `0xfc` | `0x3c` | 2,253 | 1.6% | ADMITTED |
| `G_RDPTILESYNC` | `0xe8` | `0x28` | 1,808 | 1.3% | **REJECTED** |
| `G_LOADTLUT` | `0xf0` | `0x30` | 1,808 | 1.3% | ADMITTED |
| `G_LOADBLOCK` | `0xf3` | `0x33` | 1,533 | 1.1% | ADMITTED |
| `G_SETSCISSOR` | `0xed` | `0x2d` | 932 | 0.7% | ADMITTED (state only) |
| `G_ENDDL` | `0xdf` | `0x1f` | 219 | 0.2% | **REJECTED** |
| `G_RDPFULLSYNC` | `0xe9` | `0x29` | 219 | 0.2% | ADMITTED (site only) |
| `G_SETFILLCOLOR` | `0xf7` | `0x37` | 219 | 0.2% | ADMITTED |
| `G_SETCIMG` | `0xff` | `0x3f` | 219 | 0.2% | ADMITTED |
| **Total** | | | **142,606** | | **84.0% admitted** |

Twenty-one distinct opcodes over the whole window. Nothing else appears at
all.

### 3a. What is conspicuously absent

Measured absences, each of which cancels a line of work the gap doc ranked:

- **`G_SETZIMG` (`0xfe`): zero occurrences.** Gap doc §4 ranked binding a
  depth image as item 4, reasoning that "a wrestling game is z-buffered 3D".
  In this window it never sets a depth image. Consistent with the triangle
  variant it does use: `RDP_TRI_SHADE_TEX` (`0x0e`) is the shade+texture
  variant *without* Z — the Z variants `0x09`/`0x0b`/`0x0d`/`0x0f` are all
  zero. Whether in-match gameplay differs is UNKNOWN; the abort in §2 bounds
  this window short of a match.
- **`G_SETCONVERT` (`0xec`), `G_SETKEYR`/`G_SETKEYGB` (`0xeb`/`0xea`): zero.**
  Gap doc §4 item 8 already called these optional. They are not merely
  optional here, they are unused.
- **`G_SETFOGCOLOR` (`0xf8`), `G_SETBLENDCOLOR` (`0xf9`),
  `G_SETPRIMDEPTH` (`0xee`), `G_TEXRECTFLIP` (`0xe5`): zero.** All admitted
  already, all unexercised by this title in this window.
- **Only one of eight triangle variants is used.** 10,380 shade+texture
  non-Z triangles; the other seven variants never appear.

---

## 4. The ADMITTED/REJECTED split

Classified against `WgpuBackend`'s decoder — the opcode match at
`crates/fn64-render-wgpu/src/raw_dpc/mod.rs:1066-1316`, the TMEM group at
`crates/fn64-render-wgpu/src/tmem/wire.rs:25-31`, and the width table that
gates entry at `crates/fn64-render-ir/src/command.rs:813-828`.

**119,779 of 142,606 commands (84.0%) are admitted. 22,827 (16.0%) are
rejected, across exactly three opcodes.**

| Rejected | cmd6 | Count | Why | Decoder present? |
|---|---|---|---|---|
| `G_RDPPIPESYNC` | `0x27` | 20,800 | No match arm; falls to `_ =>` at `raw_dpc/mod.rs:1310`, returning `UnsupportedCommand` | No. Width table already admits it (`command.rs:825`, `0x26..=0x3f => 8`) — only the decode arm is missing |
| `G_RDPTILESYNC` | `0x28` | 1,808 | Same | Same |
| `G_ENDDL` | `0x1f` | 219 | `raw_rdp_command_width` returns `None` (`command.rs:826`), so decode fails earlier still, with `UnknownCommandWidth` | No, and this one is arguably correct — see below |

Both rejection kinds are hard errors that abort the **entire stream**, not
per-command skips. A single `G_RDPPIPESYNC` — and every frame issues dozens —
kills the whole packet. So the honest reading of "84% admitted" is: 84% of
commands are individually understood, and 0% of frames currently survive
decoding.

`G_ENDDL` deserves a note because it is not really an RDP command. `0x1f` is
unassigned in the RDP command space; `ReferenceBackend` treats it as the
stream terminator (`gbi/stream.rs:1369`, `G_ENDDL => break`) because the game
writes it to end a submission. `WgpuBackend`'s decoder is length-delimited
instead (`while offset < stream.bytes.len()`, `raw_dpc/mod.rs:1048`), so it
does not need a terminator — it needs to not choke on one. Admitting `0x1f`
as a width-8 no-op, or trimming it before the stream is handed over, are both
valid; inventing a `G_ENDDL` semantic in the raw-DPC executor is not.

### 4a. The composition refusals matter more than the opcodes

Measured per frame, over the 218 frames with recorded deltas:

| Refusal | Frames hit | Site |
|---|---|---|
| `MixedFillAndTmemLoadPacket` | **218 / 218 (100%)** | `production.rs:1889` |
| `MixedFillAndTrianglePacket` | **152 / 218 (69.7%)** | `production.rs:1900` |

Every single WM2000 frame issues both a `G_FILLRECT` and a TMEM load, and
70% also issue triangles. The gap doc's §4 item 3 ("compose fill +
triangles + TMEM in one packet") predicted this on genre reasoning; the
census confirms it at 100%, and it is the single hardest constraint the port
faces. No amount of opcode work moves a frame through while these stand.

### 4b. Corrections to the gap doc's inventory

- `MixedFillAndTriangles` is now spelled `MixedFillAndTrianglePacket`
  (`production.rs:1900`) but is **not** fixed — both compositions are still
  refused. Renamed, not resolved.
- Gap doc §4 item 1 asked for "a `process_task` implementation, or a
  shell-side raw-DPC session + display-list-to-DPC producer", estimating the
  former against `fn64-render-reference/src/gbi/`'s several thousand lines.
  §1 above removes the display-list half of that alternative: WM2000 emits no
  display list, so no producer needs writing and no GBI front end needs
  porting. What remains is the shell-side session registration, which is a
  much smaller thing.
- Gap doc §4 item 4 (`SetZImage` + depth resolution) is not reachable from
  this window's evidence: zero occurrences, and zero Z-variant triangles.
- Gap doc §2's "which ucode WM2000 uses: UNKNOWN" is unchanged and now
  moot for this path. With zero GBI-lane commands, `supported_ucodes()`
  returning `&[]` (`production.rs:1308`) never gates anything on the raw-DPC
  route. Gap doc §4 item 7 does not apply to WM2000.

---

## 5. The smallest opcode set for a recognizable frame

**Answer: 19 opcodes, and there is no smaller one.** The gap doc's §5
hypothesis — that early screens are rectangles and blits, so the fill path is
"a substantial fraction of the actual early-frame workload" — is **half
confirmed and half refuted**, and the refuted half is the one that was going
to drive the plan.

**Confirmed: early frames really are rectangles and blits.** Frame 0 issues
60 `G_TEXRECT` and 60 `G_FILLRECT` and **zero triangles**. Triangles do not
appear until frame 66. 66 of 218 frames (30.3%) never issue one. The gap
doc's `NON-CLEAR (0 tris)` observation about five early tasks generalizes:
it holds for the first 66.

**Refuted: that does not make any frame cheap.** Frame 0 is the smallest
frame in the entire window at 366 commands, and it still needs 19 distinct
opcodes — 18 of the 21 the whole window uses. Across all 218 frames there are
only **four** distinct opcode-set signatures:

| Frames | Distinct opcodes | Difference from the 19-opcode base |
|---|---|---|
| 1 | 19 | base (no `G_LOADTILE`, no triangles) |
| 65 | 20 | base + `G_LOADTILE` |
| 133 | 20 | base + `G_LOADTILE` + triangles, no `G_TEXRECT` |
| 19 | 21 | base + `G_LOADTILE` + triangles + `G_TEXRECT` |

Commands per frame: min 366, max 907, mean 651.5.

So the minimum viable set is:

```
G_SETCIMG  G_SETSCISSOR  G_RDPSETOTHERMODE  G_SETFILLCOLOR  G_FILLRECT
G_SETCOMBINE  G_SETENVCOLOR  G_SETPRIMCOLOR
G_SETTIMG  G_SETTILE  G_SETTILESIZE  G_LOADBLOCK  G_LOADTLUT  G_TEXRECT
G_RDPLOADSYNC  G_RDPTILESYNC  G_RDPPIPESYNC  G_RDPFULLSYNC  G_ENDDL
```

Of those 19, `WgpuBackend` already admits 16. The three it does not are
exactly the three in §4's rejected table.

**What this does to the "first visible milestone" choice.** Gap doc §5
proposed "a correct WM2000 background fill in the real shell" — implement
only the background-clear prefix — on the reasoning that fill is the only
path producing a guest-visible write. The census says that milestone does not
exist as scoped. Frame 0's 366 commands are 194 fill-path commands and 172
texture commands interleaved; there is no clean prefix to stop after, and
`MixedFillAndTmemLoadPacket` refuses the packet the moment both appear. A
fill-only `WgpuBackend` would refuse 100% of WM2000 frames.

The real smallest milestone is: **fill + TMEM + texrect composed in one
packet, plus the three rejected opcodes.** That is a recognizable title
screen — 66 consecutive frames of it — and it does not require triangles at
all.

---

## 6. Two blockers found; one fixed to get this run

Recorded because the gap doc's §5 step 1 named exactly this question and the
answer turned out to be two answers.

**`MAX_TASK_STEPS` is cleared at current HEAD.** The gap doc flagged the
stale runner's abort at "RSP task exceeded deterministic 67108864-instruction
admission bound", noting the identical `panic!` still exists at
`crates/fn64-abi/src/task_dispatch/rsp_commit.rs:234-237` so being newer
"does not by itself fix it". It did. That bound never fired in any run here.

**A new blocker replaced it, and it is in the harness, not the emulator.**
The harness drove `advance_virtual_time` from a private counter seeded at
zero and stepped one VI field at a time. Guest device work advances the
fabric independently, so the counter falls behind it and
`crates/fn64-abi/src/pi/timing.rs:288-291` asserts "device time moved
backwards" — measured at step 3 of a real boot, `current=26016446` against a
requested `25015870`. Fixed by taking the target from
`max(tick, sim_time()) + field`, which keeps every target monotonically ahead
of the fabric. This is a harness bug the stale runner did not have because
the assertion did not exist then.

**The remaining blocker is the `0x1CC` MMIO read from §2**, which is what
now bounds the window at 383 VI fields. Fixing it would extend the census
into gameplay, which is the one thing this doc cannot speak to.

> **Superseded.** `0x1CC` is not an MMIO read and names no register: it is a
> KUSEG near-null address reached through a frame pointer corrupted by a lost
> shared-epilogue fall-through. Diagnosis, fix and the re-measured window
> (383 → 1,056 VI fields, 219 → 2,219 decode entries) are in
> [`RT64-WM2000-0X1CC-DIAGNOSIS.md`](RT64-WM2000-0X1CC-DIAGNOSIS.md). The
> counts in this doc remain valid for the shorter window they describe.
>
> **Superseded again.** That doc's successor named overlay-bank swapping as the
> next blocker; it is not. Bank swapping already follows the guest's own DMA,
> and the abort was N64Recomp's 40 section-local (`static_<section>_<vram>`)
> bodies, which carry the entry observer but appear in no `FuncEntry` table.
> The window is now 4,454 VI fields / 5,792 decode entries — still attract
> mode, not gameplay — and the wall has moved into the reference renderer. See
> [`RT64-WM2000-SECTION-LOCAL.md`](RT64-WM2000-SECTION-LOCAL.md).

---

## 7. Ranked: what to implement next

Ordered by frequency × cost, using the counts above. The first two items are
near-free and the third is the project.

| # | Item | Frames unblocked | Commands | Cost | Evidence |
|---|---|---|---|---|---|
| 1 | **`SyncPipe` (`0x27`) / `SyncTile` (`0x28`) as no-ops** | 218/218 (100%) | 22,608 (15.9%) | Trivial — two match arms. The width table already sizes them (`command.rs:825`); `ReferenceBackend` treats them as a no-op group (`gbi/stream.rs`), so there is no semantic to design | §4 |
| 2 | **Tolerate `0x1f`** (width-8 no-op, or trim before handoff) | 218/218 (100%) | 219 (0.2%) | Trivial, but it is a `UnknownCommandWidth` abort today, so it kills every stream regardless of count | §4 |
| 3 | **Compose fill + TMEM in one packet** | 218/218 (100%) | — | UNKNOWN. The refusal comment calls it "a follow-on slice" (`production.rs:1884-1887`) | §4a |
| 4 | **RDRAM copyback for triangle output** | — | — | Bounded — `finish_reference_task` (`fn64-render-reference/src/backend/render_backend.rs:91`) is the working precedent. Unchanged from gap doc §4 item 2: without it, a correct frame is invisible | gap doc §4 |
| 5 | **Compose fill + triangles** | 152/218 (69.7%) | — | UNKNOWN. Not needed for the first 66 frames, which is why it ranks below item 3 despite similar cost | §4a |
| 6 | **Shell can select `WgpuBackend` and register a `RawDpcAbiSession`** | prerequisite to all | — | UNKNOWN, but much smaller than gap doc §4 item 1 estimated: no display-list front end is needed, because §1 shows there is no display list | §1 |
| 7 | **`SetScissor` (`0x2d`) actually applied** | 218/218 issue it | 932 (0.7%) | Small. Admitted as tracked state only today (`raw_dpc/mod.rs:1143-1157`) | §3 |
| 8 | **`SetZImage` (`0x3e`) + depth resolution** | 0/218 | 0 | Deferred. Zero occurrences and zero Z-variant triangles in this window. Revisit only when the window reaches gameplay | §3a |
| 9 | **`SetConvert`, `SetKeyR`/`SetKeyGB`** | 0/218 | 0 | Do not build. Unused by this title | §3a |

Items 1 and 2 together cost four match arms and unblock decoding for 16.1% of
all issued commands and 100% of frames. They should land first not because
they are large but because until they do, no frame gets far enough for any
other work to be observable.

---

## 8. Re-running this as a burndown

The tooling is committed so the ADMITTED fraction can be re-measured after
each slice rather than re-argued. Re-run §2's command; the histogram is
deterministic, so any change in the counts is a change in behavior, and any
change in the admitted fraction is the slice's actual effect.

Two limits to state plainly. The census counts what `ReferenceBackend`'s
decoder dispatched, which is the right proxy for what a backend must admit
but is not itself a `WgpuBackend` run — the ADMITTED column is classification
against that decoder's source, not an observed acceptance. And the window
stops at the `0x1CC` abort, so every count here describes boot and attract,
not gameplay. A gameplay census needs that defect fixed first, and its
triangle and depth numbers could differ substantially from §3's.
