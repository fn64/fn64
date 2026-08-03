# Corpus certification: what blocks the other 282 ROMs

## Why this measurement

`gate_rom_recompile` needs one input (`FN64_DISCOVER_ROM`) and no per-game
configuration. All five AKI titles certify through it with `unsupported=0`.
The obvious question -- how much of the 287-ROM corpus certifies -- had never
been measured, so the secondary goal (all-N64 support) had no baseline.

## Result: 17 of 26 sampled ROMs certify cold (65%)

Sampled in corpus order, no configuration, no answer keys. Passing titles
include **GoldenEye**, The World Is Not Enough, 1080 Snowboarding, AeroGauge,
and All Star Tennis 99. Raw data:
`crates/fn64-discover/reference/corpus-certification-sample.tsv`.

The nine failures fall into **three classes**, not nine problems:

| class | count | shape |
|---|---|---|
| `OutsideAllMappings` | 5 | emits fully; 1-6 destinations land where no bank exists |
| `NoUniqueAdmittedTable` | 3 | overlay recovery admits no single descriptor table |
| `InvalidRangeRelations` | 1 | a recovered recipe's geometry is self-inconsistent |

## `OutsideAllMappings` is unrecovered overlays, not a missing edge case

The five affected ROMs span four unrelated engines -- Armorines (Acclaim),
Army Men 1 and 2 (3DO), Banjo-Kazooie (Rare), Bomberman Hero (Hudson) -- so
this is not an engine quirk.

Banjo-Kazooie makes the mechanism plain. It composes **one bank of 2,124
words**: a boot stub. Its unmapped destination `0x8023e620` sits far beyond
that bank's extent. The game's actual code lives in overlays the descriptor
search does not recover at all, so `OutsideAllMappings` is the *symptom*
and unrecovered overlay geometry is the *cause*. The other four show the same
shape (`0x801f9930`, `0x80214690`, `0x00292e00`, `0x802821xx`) -- addresses
past a small resident bank.

This matters for prioritization: these ROMs are not near-misses needing one
more mapping each. They need their engines' overlay descriptor formats
recognized, which is the same class of work `SearchConfig::aki_family()` and
`vrom_family()` already do for two families. `NoUniqueAdmittedTable` (3 more
ROMs) is the same root cause seen one step earlier in the pipeline.

**So roughly 8 of 9 sampled failures trace to one gap: overlay descriptor
recovery for engine families fn64 has not yet modeled.** That is a single
well-shaped research direction rather than a long tail, and the M1b result
(a 2-record floor plus contiguity recognition took corpus overlay recovery
from 32 to 41 ROMs) shows what closing one costs.

## Banjo-Kazooie: measured, and it is NOT a descriptor-table gap

Traced the one unmapped destination to its origin. The mechanism is
different in kind from AKI's overlays, which changes what closing this
class requires.

`0x8023e620` has **no pointer and no `jal` anywhere in the 16 MB ROM**. It
is constructed twice, both inside the 2,124-word boot stub, and called
directly:

```
0x80000524  lui   $25,0x8024
0x80000528  addiu $25,$25,-6624      # = 0x8023e620
0x8000052c  jalr  $25                # call into it
```

Earlier, at `0x80000450`, the stub stacks that same address alongside
`0x8003d500` and calls `0x80001d70`. That routine is a **PI DMA loader** --
its body loads `lui $4,0x100` (cart domain `0x10000000`) and
`lui $4,0x1fc0` (PIF `0x1fc00000`) and drives the PI registers.

So Banjo-Kazooie's real code is **DMA'd from cart into RDRAM by the boot
stub itself and then jumped into**. There is no descriptor table to find:
`SearchConfig`'s family search correctly reports nothing, because nothing
of that shape exists.

**What this means for the class.** The five `OutsideAllMappings` ROMs may
not share one mechanism after all -- they share a *symptom*. Banjo needs
**PI-DMA-derived bank recovery** (fn64 has `pi_dma.rs`, which slices
`osPiStartDma`/`osEPiStartDma` for constant triples; the open question is
whether this stub's hand-rolled PI writes are reachable by it), not
descriptor-family extension. The remaining four should each be traced to
their construction site before assuming a common fix; the AKI precedent of
"one fix, several titles" may not hold here.

That is a correction to this document's own earlier conclusion, made after
tracing rather than after counting error strings.

## The render/runtime lane is closed to software work

Measured from `docs/base-renderer-behavior-matrix.json` (22 behavior rows,
each carrying typed blockers):

| exactness | rows |
|---|---|
| `bounded_reference` | 18 |
| `exact_public` | 4 |
| `missing` | 2 |

Blocker kinds across all rows: **`hardware_trace` 19, `full_rom` 8**,
`implementation` 2, `allowed_spec` 1. Filtering for rows whose blockers are
neither hardware nor full-ROM leaves **zero**.

The two `missing` rows are `vi-aa-resampling-analog` (VI analog output --
DAC and composite encoding, needing physical-console capture) and
`full-rom-zero-unsupported` (needing private ROM series evidence). The 18
bounded rows are implemented and tested; they are bounded because certifying
them further means measuring real silicon, not because code is absent.

So there is no renderer or runtime work an agent can pick up. WM2000 renders
(240 captured frames, scenario gate 10/10). Effort belongs in discovery,
where blockers are still software-shaped.

## Honest scope

- 26 of 287 ROMs sampled; the wider batch was still running when this was
  written. The three classes are stable across the sample but the ratio may
  move.
- `unsupported=0` is CPU recompilation: emitted, compiled by a real `rustc`,
  run, arbitrary-PC probed. It is not a booting game.
- Certification does not require overlay geometry -- only 41 of 287 corpus
  ROMs recover any -- so single-bank titles certify without it. The failures
  above are ROMs that *need* overlays and do not get them.
