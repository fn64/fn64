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
