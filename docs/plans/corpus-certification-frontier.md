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

**Superseded by the per-ROM traces below.** This section originally read the
shared error string as a shared cause and concluded that roughly 8 of 9
sampled failures traced to one gap. Tracing each ROM individually refutes
that: Banjo-Kazooie is a compressed payload, Armorines is TLB-mapped, and only
the remaining three are the overlay-descriptor class. The paragraphs above
this one describe that class correctly; the count does not.

## Banjo-Kazooie: re-traced from the bytes, and the earlier trace was wrong

An earlier revision of this document asserted a PI DMA loader at
`0x80001d70` and a 2,124-word bank. A byte-level re-trace refutes all three
of its load-bearing claims, so they are corrected here rather than left on
record.

- **The addresses were off by `0x100000`.** Banjo is CIC-6103 (IPL3 CRC32
  `0x0b050ee0`), so IPL3 loads at `entry - 0x100000`. fn64 already implements
  this (`banks/mod.rs`, `load_delta()`) and composes the bank correctly at
  `0x80000400`.
- **`0x80001d70` is not a DMA loader.** It is exception-vector and PIF setup.
  Its `lui $4,0x100` / `lui $4,0x1fc0` are PIF-boundary constants for COP0
  helpers, not cart-domain DMA operands.
- **The bank is not 2,124 words.** It is the full 1 MiB,
  `va=[0x80000400,0x80100400)`. `pack_words=2124` is the CFG-*reachable*
  subset.

**The mechanism is implementable and recovers nothing.** Banjo's PI primitive
at `0x80002000` is fully register-parameterized, so a "recover constants at
the PI-register writes" pass finds nothing by construction; the constants live
at the caller, the shape `slice_load_request_calls` already handles. That
primitive has exactly one caller in the whole 1 MiB, and its transfer lands at
`0x8003d500..0x8005ba40` -- *inside the already-mapped boot bank*. Recovering
it adds zero banks and moves `unsupported` not at all.

**And the target is unrecoverable in principle.** The entire 16 MB ROM is
compressed (7.88-7.99 bits/byte, Rare's proprietary codec -- no Yay0/Yaz0/MIO0
magic). A whole-ROM search for `0x8023e620` finds 0 data words, 0 `jal`s, and
only the 2 lui/addiu constructions in the stub itself. It is a decompression
*output* address: its bytes do not exist in the ROM in any form a static
slicer can read. Failing closed here is correct behavior, not a gap.

## The five `OutsideAllMappings` ROMs are at least four unrelated causes

The grouping in the section above was my error -- I inferred a shared cause
from similar error strings. Tracing each ROM individually:

| ROM | reached by | actual cause |
|---|---|---|
| Banjo-Kazooie | `jalr` on a constructed address | compressed payload, unrecoverable statically |
| Armorines | `jalr` after `jal 0x80000488` | **TLB-mapped** -- `0x80000488` is COP0/`mtc0` TLB setup, and the target is a physical address behind a mapping. The same shape that ruled Perfect Dark out of the fn64 model. |
| Army Men SH | direct `jal` | overlay geometry -- 0 descriptor words at the target range |
| Army Men SH2 | direct `jal` | overlay geometry -- same |
| Bomberman Hero | direct `jal` | overlay geometry -- 0 descriptor words in `[0x80280000,0x80290000)` |

Only the last three are the overlay-descriptor class. So the claim above that
"roughly 8 of 9 sampled failures trace to one gap" is **too strong**: the
class shares a symptom, not a mechanism, and at least four distinct causes sit
under it.

## `unsupported=0` is a weaker signal than it reads as

Worth stating plainly next to every pass count in this document: GoldenEye
certifies on 41 CFG-reachable words out of an 8 MB ROM. The metric measures
whether the reachable walk stays inside a mapping -- **not** how much code was
recovered. For the all-N64 goal it will keep reporting success on ROMs where
essentially nothing was recovered, so pass counts here should be read as "no
contradiction found", never as coverage.

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

## The strongest signal: recovering overlays correlates with FAILING

Cross-referencing the certification results against
`reference/overlay-corpus-sweep.tsv` (which records admitted overlay tables
per ROM) inverts the natural assumption:

| | pass | fail |
|---|---|---|
| ROMs **with** recovered overlay tables | **0** | **5** |
| ROMs **without** any overlay table | 22 | 7 |

Every ROM that recovers overlays fails. Batman of the Future (2 tables), Big
Mountain 2000 (2), Bio F.R.E.A.K.S. (4), Bottom of the 9th (4), and
Castlevania (2) all fail; Banjo-Tooie, Blast Corps, Body Harvest, and both
Bomberman 64 titles recover nothing and certify.

The mechanism is legible in the error strings: those five fail with
`NoUniqueAdmittedTable`, `InvalidRangeRelations`, or `SourceFieldsChanged` --
every one an overlay *recipe* error, not a code-emission error. Admitting a
descriptor table commits the pipeline to completing load recipes and a
generation topology; when that cannot be completed the whole ROM fails,
whereas a ROM with no table certifies its single bank and passes.

**This is a better-shaped blocker than "246 ROMs lack overlay geometry."**
The failing set is small, the failures are typed, and they cluster in one
pipeline stage between "table admitted" and "recipes complete". The
`InvalidResidentSplit` fix earlier today was exactly this shape -- a guard in
that same stage keyed on a wrong assumption -- and it converted two AKI
titles from FAIL to PASS in one change.

Caveat on the correlation: 5 ROMs is a small denominator, and it says nothing
about the 246 corpus ROMs that recover no overlays and were never sampled.
It is a lead, not a law.

## What the recipe-stage fix reached, and where it stops

Two defects were real and are fixed. One contiguous descriptor array is
legible at every multiple of its true stride, and every such reading was
admitted as a separate table:

- **Stride aliases.** A 0x10-stride array read at 0x20/0x30/0x40 samples every
  second/third/fourth record. Each alias proposes a strict SUBSET of the dense
  table, which `canonicalize` could not see -- it collapses exact geometry
  matches only.
- **Phase-shifted readings.** Starting a coarse walk one record early reads
  neighbouring words as fields, mixing genuine borrowed records with noise, so
  the result is not a subset at all. Bottom of the 9th's table at 0x48008 has
  two records lifted verbatim from the real array at 0x48038 and one 4-byte
  "overlay" spanning ROM 0x0..0x4.

Both collapses are gated on the dense table *chaining* -- each record's
`rom_end` opening the next record's `rom_start`. Two independent arrays can
share records without either being a misreading of the other, and the chain is
what separates those cases.

Measured effect: Batman of the Future and Bottom of the 9th both reach
`admitted_tables=1`, and `NoUniqueAdmittedTable` no longer fires for them.

**They still do not certify, and the reason is not a bug.** Both now fail one
stage later, at `InvalidRangeRelations`. The recipe stage requires the
nine-word AKI-family descriptor -- ROM bounds, load start, and text/data/bss
extents, with every independent range equation agreeing. Bottom of the 9th's
records are four words (`[0, rom_start, rom_end, dest]`, stride 0x10) and
Batman's are eight. There is no text/data/bss to read; the next word is the
next record.

So the correlation this document opened with has a sharper explanation than
"recipes cannot complete". These ROMs carry a **simpler descriptor format**
than the AKI nine-word layout, and `overlay_recipe` deliberately refuses to
degrade to a linear mapping rather than guess the missing extents. Supporting
them means recovering a second descriptor schema and proving it to the same
standard -- a real project, not a guard to relax.

Big Mountain 2000 (2 admitted) and Bio F.R.E.A.K.S. (3) do not even reach that
point: their surviving tables are genuinely distinct arrays, not aliases of
one.

## Honest scope

- 26 of 287 ROMs sampled; the wider batch was still running when this was
  written. The three classes are stable across the sample but the ratio may
  move.
- `unsupported=0` is CPU recompilation: emitted, compiled by a real `rustc`,
  run, arbitrary-PC probed. It is not a booting game.
- Certification does not require overlay geometry -- only 41 of 287 corpus
  ROMs recover any -- so single-bank titles certify without it. The failures
  above are ROMs that *need* overlays and do not get them.
