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
