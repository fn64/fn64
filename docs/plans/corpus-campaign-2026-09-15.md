# Corpus campaign 2026-09-15: the NTSC funnel, measured

Status: measured result; the ranked mechanisms below are the Phase 2 queue.
Provenance: `scripts/corpus-campaign.zsh` on main-equivalent rev `6306c9d0`
(worktree-corpus-funnel, clean tree), release `fn64-discover`, jobs=3,
discover timeout 600 s, recompile timeout 1200 s. Campaign dir is private
(`~/.cache/fn64-corpus/campaign-20260915-6306c9d0`); nothing here names a
ROM path or byte.

## Funnel

| stage | passed | denominator | frontier | note |
|---|---:|---:|---:|---|
| discover | 213 | 213 | 0 | 222 files -> 213 unique normalized ROMs (1 duplicate, save files ignored) |
| pack | 193 | 213 | 20 | overlay-recovery failures are pack frontiers |
| recompile | 149 | 193 | 44 | all 44 are `unsupported_destinations` / `outside_all_mappings` |

149 of 213 (70%) certify at `unsupported=0`. Wall: discover 9.6 min,
pack+recompile 112 min; pack/recompile p50 42 s, p95 411 s, peak RSS 1.5 GB.
No resource-limit or infrastructure-failure receipts.

## Pack frontiers (20)

| kind | ROMs |
|---|---:|
| InvalidResidentSplit (generation topology) | 8 |
| InvalidRangeRelations (descriptor lacks extents) | 6 |
| NoUniqueAdmittedTable (2-4 competing tables) | 5 |
| UnalignedField | 1 |

## Recompile frontiers (44), by where the destinations land

Measured with `diagnose-cold-unsupported` (incoming-edge kind and
destination VA) plus a ROM-local placement test (session scripts, not
committed: they read ROM bytes and print only addresses).

| shape | ROMs | reading |
|---|---:|---|
| direct `call` targets in valid RDRAM outside every mapping, and a **single delta** maps >=3 of them onto `addiu sp` prologues in the ROM with a >=2x margin over the runner-up | **7** | a boot-image slice executed at a relocated VA (see below) |
| same, but the top delta ties or is under the margin | ~15 | same class, needs a stronger vote (leaf functions have no prologue) |
| targets >= 0x80800000 (beyond 8 MB RDRAM) | 14 | imprecise value sets, not missing code; no real jump lands there |
| KUSEG/physical targets from a 64-74 word loader stub | 2 | TLB-mapped execution (GoldenEye class) |
| single fallthrough off the end of the 1 MB copy | 1 | contiguous continuation |

### The relocated boot-image slice (top mechanism)

For the 7 clean ROMs (NASCAR 99, NASCAR 2000, Waialae Country Club,
Olympic Hockey 98, F-Zero X, Paperboy, NBA Showtime) the winning ROM extent
lies INSIDE the first megabyte, i.e. inside the IPL3 copy, yet the boot
image calls those functions at a different VA:

| ROM | delta | votes / runner-up | calls into segment at boot-copy VA | at relocated VA |
|---|---|---|---:|---:|
| Waialae | 0x800b90c0 | 24 / 4 | 0 | 454 |
| Olympic Hockey 98 | 0x80173670 | 10 / 3 | 1 | 151 |
| F-Zero X | 0x80390ef0 | 8 / 3 | 0 | 152 |
| NBA Showtime | 0x801225b0 | 6 / 3 | 0 | 75 |
| NASCAR 99 | 0x8006b520 | 17 / 5 | 61 | 247 |
| NASCAR 2000 | 0x80064f30 | 17 / 5 | 153 | 239 |
| Paperboy | 0x80037c20 | 8 / 4 | 454 | 368 |

The boot code materializes the copy operands as `lui`/`addiu` immediates:
F-Zero X carries the ROM offset and the RAM destination 16 bytes apart (a
PI DMA, the "descriptors are instructions" class); Waialae and NBA Showtime
carry the boot-copy source VA (a RAM-to-RAM copy of the slice).

Why discovery misses it, verified in code:

1. `delta_vote` votes only with `jal`s found INSIDE the candidate region
   (`delta_vote.rs`, `infer_region_delta`), never with the boot bank's
   proven direct calls into it.
2. The untabled strategy filters out every candidate whose ROM extent
   overlaps a baseline physical mapping (`lib.rs`, "A whole-extent proof
   SUPERSEDES ... The raw sweep is intentionally ROM-wide"), so a slice of
   the IPL3 copy can never be admitted at a second VA.
3. The physical DMA-wrapper recognizer (`pi_dma`) requires a chunked loop
   shape (backward branch, nested DMA call, cursor advance); every candidate
   on these ROMs fails all of those, because the call is a plain triple.

Proposed admission (Supported, not Proven, same bar as delta-vote): vote
sources = proven boot-bank direct-call targets that are
`outside_all_mappings`; landing sites = prologues anywhere in the ROM;
admit the unique delta with >= 3 distinct-target votes and >= 2x margin,
whose implied extent is inside the ROM and whose VA range is addressable
and disjoint from existing mappings in VA space (ROM-space overlap with the
IPL3 copy is allowed: it is the same bytes at a second address). Expected
unlock: 7 ROMs now; up to ~15 more once leaf-function landing sites
(`jr ra`-bounded starts) are added to break ties.

## Tooling follow-ups found by the run

- The sweep records the `FAILED:` prefix token ("recovering"/"building")
  as the pack frontier kind; the typed kind is in the detail text.
- `corpus-unblock-rank.py` accepts only `recompile` and `discover`; pack
  frontiers are invisible in the rank table.
- The `diagnose-cold-unsupported` JSON is not retained as an artifact; the
  receipts keep only its reason set.

## Measured outcome of the relocated-slice vote (K18, 2026-09-15)

`delta_vote::relocated_slice_vote` plus a `relocated_slice_vote` step at the
end of the untabled strategy. All 7 ROMs admit exactly one
`relocated_slice_0` mapping, `Supported` and never `Proven`, at the delta
this report predicted:

| ROM | delta | votes / runner-up / sources | admitted ROM extent |
|---|---|---:|---|
| Waialae | 0x800b90c0 | 24 / 4 / 33 | 0x4beb0..0x72000 |
| NASCAR 99 | 0x8006b520 | 17 / 5 / 24 | 0x95f4c..0xe2000 |
| NASCAR 2000 | 0x80064f30 | 17 / 5 / 26 | 0x9e710..0xe9000 |
| Olympic Hockey 98 | 0x80173670 | 10 / 3 / 18 | 0xb70f0..0xcb000 |
| F-Zero X | 0x80390ef0 | 8 / 3 / 9 | 0x72790..0x84000 |
| Paperboy | 0x80037c20 | 8 / 4 / 11 | 0xcecb4..0xe9000 |
| NBA Showtime | 0x801225b0 | 6 / 3 / 12 | 0x9d264..0x9f000 |

Every extent lies inside the first megabyte's ROM range, i.e. inside the IPL3
copy, and every VA range is disjoint from the boot bank's -- the same bytes at
a second residency, which is the whole mechanism.

### Two findings the implementation had to answer

1. **The vote sources must be the recompile gate's own authority-projected
   refusals** (`outside_all_mappings` destinations with an incoming `call`
   edge), not a broad CFG walk from candidate roots. On NBA Showtime the
   broad traversal closure offers 291 out-of-mapping targets whose top delta
   (86 votes to 70) names a ROM extent the gate's own twelve refusals give
   ZERO votes to, while those twelve pick the true slice 6 to 3. Evidence
   with no authority behind it does not merely add noise here; it outvotes
   the answer. The same substitution also flips Paperboy from 8/4 (admits)
   to 254/142 (near-tie, refuses).
2. **The extent's two ends are not symmetric.** Page-rounding the LOW end
   down claims bytes no vote reaches: on NASCAR 99 that padding pushed
   `va_start` 0xae0 bytes back into the boot bank's own VA range and the
   whole slice was refused for an overlap the rounding itself had
   manufactured. The low end is now the first voted entry exactly (already a
   function boundary); only the high end rounds out to a page, because the
   last voted entry's function body continues past it.

### What did NOT move, and why

The recompile gate's `unsupported` counts are unchanged on all 7 (153, 36,
18, 17, 12, 33, 9 before and after). A `Supported` mapping never reaches the
execution closure: `closure::ProgramGeometry` builds its `mapped` set from
`proven_bank_images()`, and `prepare_snapshot_banks_with_limits` composes a
bank only when its `bank:<name>` conclusion is `Proven`. This is a
pre-existing wall, not a property of this vote -- it already hides every
`untabled_region_*` mapping (Paperboy selects `untabled_delta_vote` and still
measures `proven_bank_count == 1`). Lifting it is a decision about what a
Supported mapping means downstream: the honest destination class is
`mapped_not_proven_code`, which is interpreter-covered `dynamic_mips` rather
than a release blocker. That is a `closure.rs` change touching every ROM and
every gate, and belongs in its own ticket.

Cost: the step composes the proven banks once to get the authority-projected
refusal set, so cold discovery on a ROM that reaches the untabled branch runs
about a third longer (F-Zero X 14.2 s -> 19.8 s, Paperboy 19.2 s -> 25.3 s,
release, same machine). Only the untabled branch pays it -- 64 of the 213
campaign ROMs -- and a ROM whose proven banks hold no raw `jal` word naming an
unmapped RDRAM address skips composition entirely, since such a ROM cannot
produce a vote source at all.

Coverage of the sources by the admitted slice, for the record: Olympic Hockey
18/18, Waialae 33/33, F-Zero X 8/9, Paperboy 15/17, NBA Showtime 10/12,
NASCAR 2000 25/36, NASCAR 99 76/153. The uncovered remainder is mostly leaf
callees below the first prologue-landing entry plus targets at or above
0x81000000, which the RDRAM filter correctly refuses to vote with.

## Measured outcome of K20 (2026-09-15)

`ProgramGeometry` now carries a SUPPORTED-mapped range set beside its proven
one, and the recompile gate composes `Supported` banks under their own names
via `compose_materialized_banks_admitting_supported_v2_with_limits`. A
destination inside a Supported bank classifies `mapped_not_proven_code`
(interpreter-covered `dynamic_mips`); one inside no bank at all is still
`outside_all_mappings`. Nothing is relabelled: block proof and owner proof
resolve their backing Proven-only, so a Supported bank contributes zero proven
blocks, zero exact owners, and zero exact-AOT or block-AOT bytes. The report
carries `supported_banks` next to `banks`.

The classification change does what B8 said it would. **The seven ROMs'
`unsupported` counts did not uniformly fall, and K20 must not be closed on
them.** Measured with the gate's own bank selection and closure scoreboard
(release, same machine):

| ROM | before | after | supported banks admitted |
|---|---:|---:|---|
| Olympic Hockey 98 | 18 | **9** | `relocated_slice_0` |
| Paperboy | 17 | **12** | `relocated_slice_1`, `untabled_region_0` |
| NBA Showtime | 12 | **3** | `relocated_slice_0` |
| F-Zero X | 9 | **2** | `relocated_slice_0` |
| NASCAR 2000 | 36 | 38 | `relocated_slice_0` |
| Waialae | 33 | 50 | `relocated_slice_0` |
| NASCAR 99 | 153 | composition ERROR | `relocated_slice_0` |

Four fall, two rise, one stops composing. Every one of those three outcomes is
the SAME finding, and it is about K18's extents, not about K20's semantics.

### Why two counts rose

Composing a Supported bank adds that bank's own CFG, and its calls reach
destinations the boot bank alone never named. On Waialae every one of the 33
prior refusals is retired -- and 50 new ones appear, all of them at ROM
offsets immediately PAST the slice's end:

    slice ROM extent 0x4beb0..0x72000, delta 0x800b90c0
    0x8012b0c0 -> ROM 0x72000   (exactly the end: a fallthrough off it)
    0x8012c15c -> ROM 0x7309c
    0x80130f08 -> ROM 0x77e48
    0x8013b62c -> ROM 0x8256c

NASCAR 2000 shows both ends of the same truncation: 25 of its 36 refusals are
retired, and 21 of the 27 new ones sit at ROM 0x9c164..0x9e608, just BELOW the
slice's start at 0x9e710. (Its remaining 6 new destinations are addresses like
0x838c8f8c and 0x8c004600, far outside RDRAM -- imprecise value sets from
decoding slice bytes that are not code at that address, the same "targets >=
0x80800000" class the funnel table already counts as not-missing-code.)

K18 chose both ends deliberately: the low end is the first voted entry exactly
(page-rounding it DOWN manufactured a boot-bank overlap on NASCAR 99), and the
high end rounds out one page past the last voted entry. That is the right rule
for the vote's own evidence, and it is measurably too small for the region the
copy actually installs. The coverage figures already recorded above said as
much -- NASCAR 2000 25/36, NASCAR 99 76/153 -- but before K20 the shortfall was
invisible, because the slice never reached the closure at all.

### Why NASCAR 99 stopped composing

    SnapshotError::UnsupportedControlDelayEntry {
        bank: "boot", entry: 0x800fcad4, control_pc: 0x800fcad0
    }

With the slice composed, a cross-bank call from `relocated_slice_0` lands at
0x800fcad4 in the boot bank, which is a DELAY SLOT. Composition refuses an
authority root in a delay slot outright, so the gate fails before it reports a
number. That refusal is correct and must not be relaxed: it means at least one
target the slice's delta implies is not a function entry at all. NASCAR 99 is
the ROM whose vote coverage was already the worst of the seven (76/153).

### The Supported/Proven split, measured not asserted

Per-bank scoreboards over the composed snapshots, confirming a Supported bank
is packed and interpreter-covered but never AOT-credited:

| ROM | bank | supported | exact_aot bytes | block_aot bytes | dynamic_mips |
|---|---|---|---:|---:|---:|
| F-Zero X | `boot` | no | 0 | 23,560 | 43 |
| F-Zero X | `relocated_slice_0` | yes | **0** | **0** | 445 |
| Paperboy | `boot` | no | 0 | 7,524 | 73 |
| Paperboy | `relocated_slice_1` | yes | **0** | **0** | 576 |
| Paperboy | `untabled_region_0` | yes | **0** | **0** | 0 |
| NBA Showtime | `boot` | no | 0 | 11,324 | 29 |
| NBA Showtime | `relocated_slice_0` | yes | **0** | **0** | 139 |

The mechanism is that block proof and owner proof resolve their backing at
`BankAdmissionV1::ProvenOnly`, so a Supported bank's blocks take a
`MissingBankBacking` blocker and no assessment is ever `Proven`. Only
composition's byte verification widens -- and that check (re-derive the mapped
ROM bytes, compare) is exactly as meaningful for a placement as for a proof.

### Verified unchanged

* `scripts/grade-all.sh`: nw4e-donor 925/0, nw4e-solo 873/0, nwxe-donor
  779/1, nwxe-solo 726/0, revenge-solo 597/0 -- byte-identical to HEAD.
* `gate-closure` measured directly: sha256 765e6349106e35b066ee28bf6b4c9e2f
  ff65bb5d8351b74dccbfec36a9291f59 -- its recorded digest.
* `gate_loaders`, `gate_selector`, `gate_delta_vote`, `gate_keys`,
  `gate_gp_base`: 10/10 byte-identical at their recorded digests.
  `gate_overlay_regions` drifts at dc7d29a8, exactly as it did at HEAD.
* `cargo nextest run -p fn64-discover`: 1103 passed, 0 failed.
* Automatic discovery admits ZERO Supported banks on WM2000, No Mercy and
  OoT, so every grader, answer-key and determinism path composes exactly the
  banks it composed before -- proved by measurement above, and structurally
  by every other composer entry point keeping `BankAdmissionV1::ProvenOnly`.

### What K20 leaves open

The semantics are in and are correct; the unlock they were supposed to deliver
is gated behind a K18 follow-up that grows a relocated slice to the extent its
own composed code implies:

1. Extend the admitted extent by fixed-point: compose the slice, take its
   in-slice calls that land outside every mapping under the SAME delta, and
   widen the extent to cover them, iterating until it stops growing. Every
   new byte is justified by a call from already-admitted code at the already-
   voted delta, so this adds no new delta hypothesis.
2. Refuse the whole slice when the fixed point implies an authority root in a
   delay slot (NASCAR 99), rather than letting composition fail the gate.
3. Only then re-measure the seven and close B8/K20's numeric claim.
