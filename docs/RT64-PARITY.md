# RT64 parity: how closely does fn64's shipping renderer match the oracle?


> **PROVENANCE WARNING.** This document's stated authority is
> angrylion-rdp-plus, which `AGENTS.md:26-45` EXCLUDES from fn64's clean-room
> protocol (`docs/DISCOVER-PLAN.md:2260` records the exclusion). Its
> observations about WM2000 and about fn64's own behaviour remain valid --
> measured facts about a ROM are explicitly allowed -- but **any claim here
> about what HARDWARE does, sourced only to angrylion, is not admissible as
> fn64 authority.** Re-ground such a claim on pinned RT64 (MIT), the public
> libultra headers, or a fresh measurement before acting on it.

This project has reported renderer progress as *ported module counts* and as
*per-card results*. Neither is a parity measurement. This doc defines one,
reports its current value, and — the part that matters most — states plainly
what the number **cannot** prove.

Every claim below is marked **CONFIRMED** (measured on this run) or
**HYPOTHESIS** (read, inferred, or quoted from another doc).

Companion docs: [`RT64-GUARD-AUDIT.md`](RT64-GUARD-AUDIT.md) (the coverage
caveat this metric is built around), [`RT64-WM2000-THREE-WAY.md`](RT64-WM2000-THREE-WAY.md)
(the real-content three-way that already exists),
[`RT64-WM2000-HARNESS-TRAPS.md`](RT64-WM2000-HARNESS-TRAPS.md).

---

## 1. The metric, stated so it cannot be inflated

> **Of N replayed cases in which RT64 is an authority, M produce a
> byte-identical committed guest framebuffer under `fn64-render-wgpu` and
> under `fn64-render-rt64`.**

Four properties make it defensible:

1. **Byte-identical, not "close".** No tolerance, no PSNR, no
   percentage-of-pixels credit. A case counts only when the two committed
   guest framebuffers are equal byte for byte.
2. **A refusal is never agreement.** If exactly one backend refuses the
   stream, that is the most consequential kind of disagreement and is counted
   as one. If *both* refuse, that is **not** parity either — neither rendered
   anything. This is pinned by `only_byte_identical_counts_as_parity`, because
   counting double refusals is the single easiest way to manufacture a
   flattering number.
3. **The denominator is stated, not implied.** See §3 for its provenance.
4. **The partitions are never summed.** See §2.

---

## 2. The partition: RT64 is not an oracle everywhere

This is the metric's load-bearing claim, and it comes from
[`RT64-GUARD-AUDIT.md`](RT64-GUARD-AUDIT.md).

**RT64 is the oracle for** command semantics, geometry, combiner and texture
behaviour. It is a genuinely separate lineage — separate authors, separate
source tree, a real GPU pipeline rather than a CPU model — which is exactly
what makes its agreement worth something.

**RT64 is NOT usable as a reference downstream of coverage.** CONFIRMED by the
guard audit, quoting RT64's own source:

- Memory alpha is hardcoded to `1.0f` under the comment **"Coverage is not
  emulated"** (`hle/rt64_blender.h:355-357`).
- There is no hidden-bits sidecar, so the RDP's real 3-bit coverage count
  stored in the RGBA16 alpha bit has nowhere to live.
- `AA_EN` and `ALPHA_CVG_SEL` are modelled **only in debugger text**.
- RGB and alpha dither are applied at a different stage with different
  arithmetic than angrylion's, and the guard audit records the authority
  question as **UNSETTLED** (its U2/U3).

**angrylion is the sole authority there.**

So a wgpu-vs-RT64 difference in a coverage-dependent case is evidence about
**RT64's modelling gap**, not about wgpu — it would look exactly like a wgpu
defect while being the opposite. A single blended percentage would therefore
be measuring the wrong thing. The runner declares an authority per case,
tallies the two partitions separately, and never adds them.

The partition is enforced in code, not by convention: a case that sets
`AA_EN` while claiming RT64 authority fails
`authority_matches_the_commands`, and any RT64-authoritative case whose
`SetOtherModes` word drifts from the no-AA/no-dither encoding fails
`authoritative_cases_use_the_no_coverage_other_modes_word`.

---

## 3. The numbers

**CONFIRMED**, measured this run. Target 320x240 RGBA16, one fill-cycle
display list per case, staged in an 8 MiB RDRAM image at `0x100` with the
color image at `0x100000` and `GUARD` half-words either side of the target.

### 3.1 Partition A — RT64 authoritative

| | count |
|---|---|
| cases (**the denominator**) | **10** |
| byte-identical wgpu vs RT64 | **9** |
| differs (both completed, bytes unequal) | **1** |
| one backend refused | **0** |
| both refused | 0 |

**9 of 10.**

*Was 6 of 10 when this document was first written.* The three
`one backend refused` cases were wgpu refusing partial-target fills
(`cannot become resident from partial initialization`); the fill lane closed
that by seeding a partial fill from the guest's own framebuffer, and closed
the scissor gap alongside it. All three now render and match the oracle
byte-for-byte. That is the first time a change in this project was **scored**
rather than asserted -- the instrument moved, and the number moved with it.

One integration defect surfaced only where the two lanes met, and is worth
recording because it looked exactly like a renderer regression: the fill work
added a `guest_rdram` field to `ConformanceReplay`, and this runner -- merged
from a separate lane -- did not set it. Setting it to `None` compiled, but then
the replay supplied 0 sources for the 1 read a partial fill now declares, so
every partial fill was refused and parity read **2 of 10**. The runner already
built an RDRAM image two lines above; it simply was not passing it. A parity
drop is not automatically a backend finding.

### 3.2 Partition B — RT64 NOT authoritative (coverage / AA / dither)

| | count |
|---|---|
| cases | **2** |
| byte-identical | 1 |
| one backend refused | 1 |

**Reported, never added to Partition A.** A difference here is not a wgpu
finding.

### 3.3 The denominator's provenance — read this before quoting §3.1

**The 10 is hand-authored.** Every case is a display list this lane wrote to
probe one specific behaviour. **A number over ten hand-chosen cases is nearly
meaningless as a fidelity estimate**, and it is not offered as one. It is a
*regression instrument* with an honest partition and a real oracle behind it.
Hand-chosen fixtures test what the author imagined; only captured content
tests what the game actually draws. See §5.

---

## 4. Ranked disagreements

Disagreements are **output, not work** — this lane does not fix them.

### 4.1 `scissor-top-rows-only` — wgpu ignores the scissor on Y. CONFIRMED.

**The strongest finding here, and it is corroborated three ways.**

A fill asks for the whole 320x240 target while `SetScissor` admits only the
top 120 rows. **38,400 pixels differ — exactly 320x120, the whole
scissored-out region.** At the first excluded pixel (x=0, y=120) RT64 leaves
the seeded `0xffff` and wgpu paints `0x07c1`.

Both **RT64 and `fn64-render-reference` match the hand-derived key; wgpu alone
does not.** Two independent lineages plus an independently derived key all
agree against wgpu, so this is not a key error.

Note that `scissor-narrower-than-rect` — the same shape on the **X** axis —
is byte-identical. **wgpu honours the scissor horizontally and ignores it
vertically.** That asymmetry is the actionable detail for whoever fixes it.

### 4.2 Partial-target fills are refused by wgpu. CONFIRMED.

`top-left-quadrant`, `single-pixel`, and `last-column-last-row` — every fill
that does not cover the whole target — are completed by RT64 and refused by
wgpu:

```
color target ... cannot become resident from partial initialization
TargetRectangle { x: 0, y: 0, width: 160, height: 120 }
```

RT64 renders all three and matches the key on all three. **This is one guard,
not three defects**, and it is the largest single contributor to the 4
non-identical cases in Partition A.

> **Owned by another lane.** The fill/scissor path
> (`crates/fn64-render-wgpu/src/targets/fill.rs`) was live under a different
> card while this measurement ran. Both 4.1 and 4.2 land in that path.
> Reported here, deliberately not touched.

### 4.3 `scissor-narrower-than-rect` — the key is the outlier, not the backends. CONFIRMED.

RT64 and wgpu are **byte-identical**, and **both disagree with the
hand-derived key**, which `fn64-render-reference` matches.

This is the one row where the oracle *vindicates* wgpu, and it is why the
runner reports per-backend key agreement rather than just a pairwise count.
The guard audit records (its §on scissor rounding) that RT64 and angrylion
genuinely disagree on **subpixel scissor rounding** — angrylion clips exactly,
RT64 snaps outward with `(x + 3) >> 2`. **HYPOTHESIS:** that disagreement is
the cause here. Settling it needs angrylion, which this lane did not run.

---

## 5. Corpus provenance, and whether real captured streams are reachable

**Yes — and the path is already committed. CONFIRMED.**

- `FN64_GBI_PACKET_DUMP` emits `entry \t lane \t pc \t w0 \t w1` TSV from
  `fn64-render-reference`'s `gbi::census::packet`, hooked into the decoder at
  `crates/fn64-render-reference/src/gbi/stream.rs:266`. That is exactly a
  `Vec<(u32, u32)>` plus the RDRAM address it was read from.
- `FN64_XBUS_STREAM_DUMP_DIR` writes big-endian `xbus-NNNN.bin` streams and
  optional full RDRAM images (`crates/fn64-abi/src/task_dispatch/rsp_commit.rs:1938`).
- The production wgpu plan/execute/publish session does not traverse that
  legacy XBUS hook. `FN64_RAW_DPC_STREAM_DUMP_DIR` captures its owned raw-DPC
  submissions instead, with `FN64_RAW_DPC_STREAM_DUMP_SKIP` and
  `FN64_RAW_DPC_STREAM_DUMP_COUNT` selecting a bounded index window and
  `FN64_RAW_DPC_STREAM_DUMP_RDRAM` selecting the sole index allowed a full
  RDRAM image. Each stream is canonical big-endian words plus a source/range
  metadata sidecar. All outputs remain outside git.
- The parity runner now reads the TSV form directly (`FN64_WM2000_PACKET_TSV`,
  `FN64_WM2000_PACKET_ENTRY`) and reports the captured packet's size, command
  count, and **its own** target extent and destination — read from the
  packet's `SetColorImage`/`SetScissor`, never guessed. Guessing the width is
  the documented cause of "striping" misreported as a renderer defect three
  times ([harness traps](RT64-WM2000-HARNESS-TRAPS.md)).

**Nothing is committed, and that is deliberate.** A game's own RDP command
words are game content, which `README.md`'s "no game content ships in this
repo" rule covers. With the variable unset the report says
`captured_corpus.available: false` and gives the reason, rather than quietly
presenting a synthetic number as though real content backed it.

**A ROM run is required to produce a capture, and this lane did not perform
one** — one ROM at a time is a hard rule and another lane may need it. **Left
for the controller to schedule.**

**Prior art, and it is strong.** [`RT64-WM2000-THREE-WAY.md`](RT64-WM2000-THREE-WAY.md)
already replayed a **real captured WM2000 frame-0 packet** through all three
backends and reports **0 of 115,200 pixels differing for all three pairings**,
with `alpha_dither` controlled to `Disabled`. **HYPOTHESIS (quoted, not
re-measured here):** on real WM2000 frame-0 content wgpu and RT64 are already
at exact parity. That result and §3.1's 6-of-10 are not in tension — they
probe different things, and §6 says why.

---

## 6. What this number CANNOT prove

The most important section. The temptation to over-read a clean partition is
exactly the failure mode this doc exists to prevent.

1. **It is not a fidelity estimate.** Ten hand-authored fill-cycle cases are
   not a sample of anything. They were chosen to probe specific behaviours, so
   the ratio reflects the author's imagination, not the renderer's coverage of
   real content.
2. **Agreement with RT64 is not agreement with silicon.** RT64 is a separate
   lineage, which makes its agreement meaningful, but it is still an
   implementation. Both could be wrong together — §4.3 shows both disagreeing
   with a hand-derived key on the very same row.
3. **It says nothing about anything downstream of coverage.** Anti-aliasing,
   coverage-dependent blending, and dither are excluded *by construction*. A
   perfect Partition A score would leave those entirely unmeasured. Only
   angrylion can settle them.
4. **It covers fill cycle only.** No triangles, no textures, no combiner
   modes, no TMEM, no multi-cycle blending, no VI. The overwhelming majority
   of what a real frame does is outside this corpus.
5. **It is a single-frame, single-packet comparison.** No frame history, no
   inter-frame state, no swap behaviour.
6. **A refusal counted against parity is not automatically a wgpu defect.**
   A guard may be correctly protecting a real invariant; the harness traps
   record several refusals that were right. §4.2 is reported as a
   disagreement to investigate, not a proven defect.
7. **It cannot distinguish "not implemented" from "implemented differently"**
   for a refused case, because a refused case produces no pixels to compare.

---

## 7. Reproducing

**CONFIRMED**: deterministic — three consecutive runs produced a
byte-identical JSON document.

```sh
# RT64's C++ build needs its MIT source tree. The crate default resolves
# relative to the crate manifest and does NOT resolve from a /private/tmp
# worktree; this is a worktree-location fact, not a missing dependency.
export FN64_RT64_DIR=/Users/jer/Code/no-mercy-recompiled/third_party/rt64
export CARGO_TARGET_DIR=/private/tmp/fn64-parity-target   # keep out of the repo

cargo build -p fn64-render-conformance --features parity-runner \
  --bin fn64-render-conformance-parity-runner

# stderr carries RT64's native device banners; stdout is pure JSON.
"$CARGO_TARGET_DIR/debug/fn64-render-conformance-parity-runner" 2>/dev/null \
  | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin)["parity"], indent=1))'
```

With a capture available, add:

```sh
export FN64_WM2000_PACKET_TSV=/path/to/packet-dump.tsv
export FN64_WM2000_PACKET_ENTRY=0
```

---

## 8. Verification

| Check | Result |
|---|---|
| `cargo nextest run --workspace --offline` | **8664 passed / 13 skipped** — the stated baseline, unchanged |
| Parity runner unit tests | **18 passed** |
| Feature off (default features, `FN64_RT64_DIR` unset) | crate builds and tests clean; the runner compiles to nothing |
| Determinism | 3 consecutive runs, identical output digest |

### 8.1 Mutation evidence

The instrument's ability to **detect** a disagreement was mutation-tested
rather than assumed — including the arms kept, not only the ones changed. All
six mutants were killed.

| Mutant | Expected kill | Observed |
|---|---|---|
| M1: `wgpu_bytes` returns RT64's bytes | every case becomes identical | Partition A went 6/10 → **10/10**, all refusals vanished (measured against the 6/10 baseline of the day; the mutant's meaning is unchanged at 9/10) |
| M2: invert the `scissor-top-rows-only` key | parity tally **unchanged**; only key attribution moves | tally identical (6/10 at the time, 38,400 px); `rt64_matches_key` true → **false** |
| M3: declare the AA case RT64-authoritative | the partition guard refuses it | **two** tests failed independently |
| M4: count `BothRefused` as parity | the parity definition refuses it | `only_byte_identical_counts_as_parity` failed |
| M5: drop the packet-dump contiguity check | a gapped dump must be refused | `captured_parser_refuses_a_gap` failed |
| M6: `walk` gives `G_TEXRECT` 8 bytes instead of 16 | the stream desynchronises | `captured_walk_gives_texrect_sixteen_bytes` failed |

**M2 is the one that matters most.** Inverting the key left the reported
parity numbers *completely unchanged* while flipping which backend the key
blesses. That proves the parity number depends only on the two backends and
that the key is a genuinely independent third authority used purely for
attribution — not something derived from either backend to make it look good.
