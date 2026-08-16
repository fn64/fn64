# Deterministic ROM Discovery Plan

Status: active implementation plan
Last updated: 2026-08-01

Execution handoff: [`plans/discover-pipeline-improvements.md`](plans/discover-pipeline-improvements.md)

## Composing milestone: byte-exact rebuild proven, known + novel (2026-08-01)

`gate_rom_rebuild` composes automatic discovery into the Phase-8 end
artifact: every proven bank materialized, every proven code region emitted as
GNU `as` text, assembled/linked at its VA, byte-compared, and the verified
bytes written back into a ROM image whose sha256 must equal the original's.
Two granularities carry two claims — exact owners round-trip as functions
(the decompile claim), maximal contiguous proven-block runs round-trip with
no ownership claim (the code-classification claim). Unfaithful words
(out-of-region branches, non-canonical encodings, embedded table data) are
retained as numeric literals, counted, and reported.

First results, cold discovery, zero reference TOMLs, run-twice
byte-identical, digest match everywhere:

- **Majora's Mask (USA)** — the known-recomp target (zeldaret / Zelda64Recomp
  ground truth grades the decompile side separately): **605 banks, every
  region exact, 101 raw words total**, 66,312 physical + 2,173,380
  materialized bytes round-tripped. The all-bank answer-key grade on this
  tree is **16,443 / 17,108 exact, 2 coarse/interior, 663 open, 0 wrong**
  (boot 402/486; overlays 13,001/13,016; code segment 3,040/3,606).
- **Clay Fighter 63⅓ (USA)** — the novel target, picked mechanically by
  `scripts/pick-novel-rom.py` from complete `gate_rom_rebuild` receipts using
  the declared maximum-absolute-roundtripped-code metric (no known
  decomp/recomp project): **220/220 regions, zero raw words, 811,944 bytes**
  of cold-proven code round-tripped in one bank.
- OoT (USA), Buck Bumble, Penny Racers, Automobili Lamborghini, Ridge Racer
  64 also pass; invocations and the full table live in
  `crates/fn64-discover/reference/corpus-invocations.md`.

Honest limits: exact owners are zero in the cold rebuild snapshots
(functions=0/0
everywhere), so the decompile-grade claim still rides on
`gate_decomp_functions`' wrong==0 catalog; opaque bytes remain the explicit
frontier (93.51% on Clay Fighter after separately classifying its 4,096-byte
header/IPL3 — assets, data, and unproven code); and
materialized (compressed) bank sources stay original bytes in the rebuild,
with their round-trip proven on output bytes.

## Objective

Turn a normalized N64 ROM into enough bank, code, function, relocation, and
ABI metadata to decompile and statically recompile it without hiding uncertain
regions. The normal path starts with ROM bytes only. A versioned external
manifest may supply clean-room evidence that automation has not recovered yet,
but the generic engine contains no ROM-ID dispatch or per-game constants.

This plan tracks three different outcomes:

1. **ROM understanding:** every byte is classified as code, table, asset,
   compressed content, padding, conflict, or explicitly unknown.
2. **Static recompilation:** every required code byte has a bank-qualified
   owner, callable entries, exact extent, and closed direct transfers.
3. **Runtime fidelity:** the emitted program passes deterministic runtime
   video/audio/input comparisons. Static discovery success does not imply this.

Full-game recompilation has a stricter closure condition than high static
coverage: every guest CPU transfer must have an executable destination. Each
bank-qualified address is therefore classified as one of:

- `exact_aot`: admitted through exact function-owner proof;
- `block_aot`: emitted from a proven executable basic-block graph without
  claiming an original source-level function boundary;
- `dynamic_mips`: executed by an explicit, instrumented MIPS fallback when
  bytes or targets are produced only at runtime; or
- `unsupported`: a loud, release-blocking frontier.

A full-game gate requires zero `unsupported` destinations. `dynamic_mips` is
never silent: every entry records bank, PC, byte identity, source mapping, and
reason AOT admission failed. A pure-static build additionally requires zero
dynamic fallback, but the generic all-ROM architecture does not depend on
that stronger condition being mechanically provable for every program.

## Current measured baseline

### ROM-only cold coverage panel (2026-07-31)

`scripts/cold-coverage-panel.py` is the path-free breadth gate. A strict
external manifest names canonical regular ROM files and their expected
normalized digests. Every image runs in an environment-cleared process group
with a 600-second timeout, a sampled 2 GiB aggregate-RSS watchdog, a 40%
system-free-memory floor, and kernel-enforced 1 MiB stdout/stderr limits. Linux
also receives a per-process address-space backstop; macOS records that no
reliable hard memory limit is available. The parent verifies the complete
`fn64.cold-rom-measurement.v2` shape, internal totals, and normalized identity,
verifies the receipt digest, and binds and revalidates the exact
executable digest; it publishes no partial output.
Wall time, peak RSS, manifest IDs, and paths do not enter the deterministic
receipt.

The v2 gate covered nine private ROMs from five engine families. Ten complete
sweeps emitted byte-identical per-ROM receipts. The sealed panel and artifact
digests remain with the private result rather than becoming an ungated
repository assertion. Complete-panel wall times were 118.225–119.010 seconds
and the observed process-group peak sample was 577,765,376 bytes.

| ROM | selected strategy | banks | facts | exact AOT destinations | block AOT destinations | dynamic destinations | unsupported destinations | code-like floor bytes |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| No Mercy (NW4E) | recovered overlays | 6 | 31,457 | 0 | 1,820 | 11 | 0 | 0 |
| WrestleMania 2000 (NWXE) | recovered overlays | 5 | 21,528 | 0 | 1,810 | 13 | 0 | 0 |
| Ocarina of Time | recovered VROM | 471 | 104,826 | 0 | 819 | 10 | 0 | 8,192 |
| Majora's Mask | recovered VROM | 605 | 81,839 | 0 | 1,527 | 15 | 0 | 81,920 |
| Kirby 64 | recovered overlays | 10 | 23,898 | 0 | 2,865 | 32 | 0 | 704,512 |
| GoldenEye 007 | boot bank only | 1 | 5,925 | 0 | 3 | 1 | 0 | 32,768 |
| Perfect Dark | boot bank only | 1 | 339 | 0 | 3 | 1 | 0 | 0 |
| Super Mario 64 | untabled delta vote | 1 | 14,241 | 0 | 163 | 0 | 0 | 245,760 |
| Banjo-Kazooie | boot bank only | 1 | 223 | 0 | 363 | 0 | **1** | 0 |

The sole static release blocker is Banjo-Kazooie's one
`outside_all_mappings` destination. Kirby 64 has the largest measured residue
and dynamic tier. `ledger_code_like_floor_bytes` is exactly
`undiscovered_code_bytes()`: the measured CodeLike heuristic floor, not a
complete undiscovered-code total. A zero floor on a boot-only model is not a
whole-game closure claim.

An earlier v1 panel is historical only. Its zero runtime-event count was
vacuous because the static measurement executed no guest code, its raw receipt
artifact was not retained, and its `7a98bc2d…68a6` transcript hash cannot be
recomputed. V2 removes that field and reports static `unsupported` only over
the exact `total_destinations` denominator. It also retains all four class
tallies and all eight reason buckets, including zero-valued buckets.

The retained v2 baseline contains 35,646 `OverlayRelocation` facts for OoT and
46,993 for MM; the other seven ROMs contain none. Session output recorded
pre-ingestion totals of 69,180 and 34,846 facts and unchanged closure/ledger
scores, but no raw pre-ingestion receipt survived. Those deltas therefore
cannot be independently recomputed and are not admitted as a verified panel
delta. The canonical facts remain inert evidence rather than callable roots.
Any later consumer must prove both the parser-derived value and its role;
`R_MIPS_32` plus a text-range value is not enough after the measured
25-function split failure in the answer-key gate.

Stage-1 effect classification now exists independently of the WM-specific
source-frontier gate. `stage1_effects::scan_stage1_effects` inventories raw
COP0/cache/sync/trap encodings, qualifies code versus data-shaped words with
the authority CFG, and performs a sound but deliberately local constant-
address pass that resets at every basic-block entry. Direct KSEG0/KSEG1 aliases
are converted to physical addresses before RDRAM/RCP/PIF classification;
KSEG1 by itself is not an MMIO signal. Every cross-block, TLB-dependent, or
path-ambiguous memory address remains open, so an empty positive list would
still not be a purity theorem.

The nine-ROM scan measured reachable obvious external-effect sites (intrinsic
COP0/cache/sync/trap plus exact RCP/PIF accesses) as NW4E 136, NWXE 136, OoT
100, MM 107, Kirby 146, GoldenEye 6, Perfect Dark 6, SM64 37, and Banjo 34.
Exact RCP accesses were respectively 68, 68, 46, 40, 79, 0, 0, 5, and 10;
no exact PIF access appeared. More importantly, unresolved reachable memory
addresses remained 2,617 / 2,585 / 1,427 / 2,380 / 4,688 / 2 / 2 / 273 / 493.
This validates the sequencing decision: stage 1 cheaply rejects many blocks,
but cannot yet mint the effect/closure certificate required to admit block
harvest. Typed-memory separation and complete call/effect propagation remain
later proof obligations.

`fn64-discover study-layout <rom> <dump.toml>` is a held-out research
diagnostic, not a cold input. It seals the ROM-only receipt before opening a
bounded answer key, reruns discovery/composition under the receipt's exact
limits, requires the scoreboards to agree, and binds the answer-key digest into
its own receipt. `FN64_DISCOVER_PRINT_GRADES=1 gate_decomp_functions` emits the
corpus-only adjacency rows used by that line of research. One real OoT run
completed, but cold discovery had zero exact owners, so it produced zero
candidate gaps. The command has not passed a 10-run characterization and the
layout hypothesis remains untested by the baseline.

Accordingly, no sealed block-harvest evaluation was run and no evaluated image
was admitted as writer-class authority. The reserved
`ValidatedWriterClassReceiptV2::CpuCopyStoreOrDecompression` shape is now
structurally split into three independently retained receipts: an
effect/closure certificate, an exact evaluated-image block-harvest receipt,
and class-completeness aggregation. The production constructor remains absent.
This prevents a future successful execution sample from substituting for
either all-executions closure or completeness of the writer-class inventory.

The depth-side anchor is the separate 100k exact-entry AOT/dynamic diagnostic,
attempt 25, landed in `f527f08`. Both lanes completed 100,001 guest
instructions and matched RDRAM, CPU, device, executor, ABI-host, continuation,
scheduler-step (33,333), and simulated-time state. Both publication diagnostics
retained the same pending executable write, last charge 5, and cumulative
instruction count. It bounds one exact fallback transition; it does not improve
any cold-panel coverage number above.

### Cold training, then label-free application

Known recompilation/decompilation corpora are training data, not inputs to the
production discovery engine. A valid training run has two ordered phases:

1. Run discovery from the ROM alone and publish a schema-v4 snapshot workspace
   with `intended_use = sealed_cold_function_training_input` and
   `answer_key_present = false`. Its snapshot wire is v6 and its candidate
   receipt is v3. The addressed identity includes `RomAddressSpace`, so the
   same numeric ROM/VA pair in physical ROM and VROM is not collapsed; V3 also
   includes typed semantic-callable authority in the fixed detector denominator.
2. Stream-validate the complete fixed workspace namespace, manifest, candidate
   receipt, bank bytes, snapshot digests, geometry, and ROM binding. Only after
   that validator returns the sealed identity may a grading process open a
   label file. The labels classify every known row as candidate-matched,
   missed, ambiguous, out of modeled scope, or invalid input and cluster the
   misses by causal evidence. A candidate match is not a proven function
   owner or extent. Labels never alter the already-sealed candidates.

This ordering is the minimum credible cold-start test. The historical
`gate_decomp_functions` remains useful as a regression and mechanism probe,
but it is **not** a cold baseline: it reads the answer key and derives
`code_end` from labeled function extents before discovery. Its scores must not
be presented as ROM-only application performance.

Training may use several labeled ROMs to identify recurring miss classes and
choose a general mechanism. Application is evaluated leave-one-ROM-out: seal
the target ROM's cold workspace without its key, select/freeze the mechanism
using only the other ROMs, then open the held-out labels for grading. The
production path for a new ROM stops before that final label step and receives
no target-specific address, extent, name, or game constant.

A 2026-07-30 cached OoT characterization of the new cold producer completed in
about 46 seconds at 448 MiB peak RSS and occupied about 106 MiB outside Git.
The workspace contained 471 banks, 5,721,664 materialized bank bytes,
98,032,529 aggregate snapshot-artifact bytes, and a 5,263,929-byte candidate
artifact. Key-free streaming validation visited 107,534 fact rows and 12,966
candidates (zero ungradable) in about 0.85 seconds. The sealed identities were:

```text
normalized ROM SHA-256       c916ab315fbe82a22169bff13d6b866e9fddc907461eb6b0a227b82acdf5b506  (private receipt; not tested here)
workspace manifest SHA-256  23e9a4f1204a8fdacbb2dea26ede6f04e668672d658c83d52f21dcf0a2e88be2  (private receipt; not tested here)
candidate artifact SHA-256  72827994abfc670e61b5e7e98f17debaf1b03218a9c5c7a662d6fe1d5ad53f7d  (private receipt; not tested here)
candidate identity v2       <historical private receipt; cannot authorize v3 grading>
```

These are performance and input-integrity measurements, not a function-recall
result. An answer-key row count is also not automatically a CPU-function
denominator: aliases may repeat an extent, zero-size linker markers are not
bodies, and a dump may contain RSP or CIC code alongside VR4300 code. Until
the grader records a typed execution domain, dump-only rows remain `Unknown`;
zero-size markers are preserved for audit but excluded from the body/miss
denominator. Consequently neither the raw OoT dump count nor the 12,966 cold
candidates establishes whole-ROM or all-code-path coverage.

The subsequent grading-only pass exhaustively accounted for all 12,966 cold
candidate identities and all 13,358 answer rows. Of 13,352 nonzero rows,
12,802 matched a cold candidate (95.88% candidate recall, not ownership or
extent proof); the candidate union contained 12,802 exact known-entry matches,
135 function interiors, 29 outside candidates, and zero ungradable or
detector-only omissions. The remaining rows clustered into 547 mapped
`no_relation` and three `proven_code_no_entry`; no answer row remained
unmapped. A cached pass took about 1.15 seconds at 283 MiB peak RSS and
produced a 30.7 MiB report. These are private characterization measurements
and are not tested by the repository.

Two general mechanisms produced that delta. A bounded request-DMA fixed point
found one additional whole-file bank from exact operands in an already proven
loaded bank, eliminating all 211 former `n64dd` mapping misses. A
candidate-only o32 argument-home-spill leaf detector added 254 exact OoT entry
matches without adding an interior/outside candidate. On held-out NWXE and
SM64 cold workspaces it emitted one and four candidates respectively; all five
matched known entries and none were interior/outside. That is observed
cross-family evidence, not a universal precision proof.

The next training-ranked mechanism reuses the existing answer-independent
dense/fixed-stride handler-table scanner instead of inventing another pointer
heuristic. An independent replay over the sealed OoT bank bytes found 2,735
candidates: 2,632 exact known starts, 103 non-answer starts, and 127 of the 550
current misses (96.23% measured candidate precision and 23.09% of the remaining
miss set). Production harvesting now records this pattern with distinct typed
candidate evidence and scans each complete bank while it is already
materialized, before executable-range slicing, so it retains no second
ROM-sized copy. The 103 non-answer starts forbid authority: the provider does
not emit an identified-table fact, prove a consumer/index domain, or seed the
authority closure. A fresh key-free schema-v4 OoT run reproduced the projected
delta exactly: 12,929/13,352 bodies matched candidates (96.831935%), leaving
420 `NoRelation` and three `ProvenCodeNoEntry` misses. The union contained
13,196 candidates: 12,929 exact starts, 238 interiors, zero gaps, 29 outside,
and zero ungradable; all 103 newly admitted non-answer identities were
interiors. The producer peaked at 469 MiB, validator at 142 MiB, and attribution
at 185 MiB. Digest-pinned private receipts bind the local artifacts; repository
tests do not retain or recheck them. A frozen, key-free held-out SM64 application then
recovered 30 additional exact bodies: 2,872/3,891 became 2,902/3,891
(73.8114% to 74.5824%), with 37 new unique candidates split into 30 exact,
two interiors, zero gaps, and five outside. The provider emitted 346 SM64
candidates in total; 309 corroborated an existing detector. Producer peak RSS
was 296 MiB. Digest-pinned private receipts bind the held-out artifacts;
repository tests do not retain or recheck them. This establishes cross-family
candidate utility, not callable authority or universal precision.

A bounded prologue-prefix refinement then moved an existing prologue candidate
back by at most three plain setup words only at an independent terminator or
two-NOP boundary, rejecting zero/control/`$sp`-writing prefixes and direct
entries into the intervening words. A fresh key-free OoT run moved 99 interior
candidates to exact structural boundaries and recovered seven additional
bodies without losing any previously matched body: 12,936/13,352 matched and
416 remained missed. The candidate union shrank from 13,196 to 13,104 because
relocated entries deduplicated with existing claims; interiors fell from 238
to 139, while gaps and outside candidates remained zero and 29. Digest-pinned
private receipts bind this run; repository tests do not retain or recheck them.
The same frozen mechanism applied key-free to held-out SM64 recovered two
additional bodies, lost none, and reduced interiors by 33 (402 to 369). This
remains candidate refinement, not callable authority.

Typed semantic-callable publication then preserved the callback/thread
contracts already derived by authority closure instead of retaining only their
root addresses. Snapshot v5 carries the target, caller site, callee, pointer
register, and contract, and candidate receipt v3 exposes the resulting Proven
entry with a distinct detector. A fresh key-free OoT run added exactly one
candidate-matched body (12,936 to 12,937), reduced misses from 416 to 415 and
`ProvenCodeNoEntry` from three to two, and introduced no interior, gap,
outside, ungradable, or matched-to-missed delta. The same frozen mechanism on
held-out SM64 changed no candidate or body status. The historical-to-current
A/B is explicitly a cross-schema unprojected total delta, so the synthetic
typed-evidence tests establish the causal mechanism while the ROM runs bound
its observed gain and absence of held-out regression. This closes one lost
provenance path; it is not a broad function-discovery mechanism.

The tempting next shape -- one missed answer body between two matched answer
bodies -- was rejected as an implementation target. It described 236 bodies
only because answer sizes supplied the partitions. The actual label-free rule,
using one candidate owner's proposed end and the next candidate entry, emitted
61 gaps: one was an exact body, two began correctly with a wrong end, 51 split
real bodies, and seven were padding or otherwise unmatched. Neither flank had
a proven owner for any of the 236 answer-described cases. Exact-gap promotion
therefore cannot be made sound from current facts; progress must instead add
typed callable-entry authority such as exhaustive computed/callback/table or
cross-generation flow.

A subsequent SM64 cold training experiment found a promising chunked
physical-ROM wrapper candidate without embedding a game symbol or address.
Temporary direct promotion mapped physical ROM `0x0f5580..0x108a10` at VA
`0x80378800..0x8038bc90` and moved all 200 engine bodies out of `NoMapping`:
178 matched candidates and 22 became `NoRelation`; the candidate union added
178 exact matches, 50 interiors, 21 gaps, and zero outside candidates. Held-out
NWXE and OoT controls admitted no extra mapping. Independent review then found
that the classifier was path-insensitive, did not authenticate its nested PI
callee, and did not relationally prove that one chunk value advanced both
cursors and reduced the remaining length. Production promotion has therefore
been disabled: these numbers measure potential payoff, not current authority
or recall. The next admissible increment is CFG-aware relational proof plus an
independently proven PI primitive. Request-DMA and wrapper open counts and
limit flags now survive automatic discovery and the sealed workspace manifest;
the 64-input bound retains a deterministic scanned prefix and reports the
withheld suffix rather than abandoning all progress.

A follow-up authority audit stopped an unwired loop verifier before retention.
The real candidate is only a `Supported` entry, has no covering proven
executable interval, and is absent from the authority CFG. Its inner primitive
also cannot be authenticated from an entry prefix: a sound capability needs a
complete public-libultra EPI/send contract, independently proven cart singleton
and PI base, and call-site message/queue alias effects. A verifier over raw,
independently supplied bytes, CFG, and facts would not bind those authorities.
The next implementation should therefore start with a digest-bound prepared
bank and entry/range authority; loop-step code with no legitimate positive
fixture is intentionally not retained. This makes the SM64 cluster a later
proof-chain target, not the next production-recall increment.

`gate_d1` grades candidate function starts only. It does not grade extents or
byte ownership.

| Corpus/input | Candidate precision | Entry recall | Important limitation |
|---|---:|---:|---|
| OoT, overlay-only held-out grade | 99.5672% | 72.3312% | 469 gate load images; the full mechanical path has 470 including resident `code`, while `n64dd` remains open |
| NW4E, descriptor mapping | 48.4387% | 89.7384% | mapped data is still scanned as code |
| NW4E, descriptor plus text intervals | 82.4089% | 88.1105% | text intervals are external evidence, not inferred yet |
| NWXE, boot mapping only | 36.3969% | 28.5422% | overlays absent from this preserved baseline |
| NWXE, mechanically recovered overlays | 49.9764% | 86.8960% | four ROM-only recovered mappings; mapped data is still scanned as code |
| NWXE, descriptor mapping only | 49.9529% | 86.8550% | overlay data is scanned as code |
| NWXE, descriptor plus text intervals | 81.3143% | 84.1114% | text intervals are external evidence, not inferred yet |

The NWXE text filter reduced combined candidates from 4,246 to 2,526 and
false positives from 2,125 to 472. The 2.74-point recall reduction came from
removing data words that happened to decode as calls to real function starts;
it is not evidence that the excluded bytes are code.

The same held-out operation on NW4E raised precision by 33.97 points while
losing only 1.60 points of entry recall. This is independent confirmation that
load-image and executable-region recovery should run before more elaborate
function detection.

A first cross-ROM relocation-masked whole-body index was then graded without
using target function boundaries during matching. NWXE functions proposed 552
unique NW4E targets, 550 of which were exact starts (99.6377% precision and a
15.9884% lower bound on target entry recall). The reverse direction proposed
560 unique NWXE targets, 553 exact (98.7500% precision and a 22.6454% recall
lower bound). Repetitive normalized bodies were reported as ambiguous rather
than selected. CFG-structure matching and external similarity tools should
therefore concentrate on the unmatched remainder. Ten consecutive bidirectional
gate runs produced the same output SHA-256
`d808f8916d66dee749244d7b15912525566daec1dc3b10d48d44697d76431290` (no test
re-checks this historical digest: its evidence-manifest inputs were
session-local and are not preserved on disk).

An unseeded spimdisasm 1.42.2 run over only NWXE's externally supplied
resident text interval proposed 899 entries against 847 known entries. It
found 827 exact starts (91.9911% precision, 97.6387% recall). Of the common
starts, 666 extents were exact (80.5320%), 153 were too long, and eight were
too short. This establishes spimdisasm as a high-value candidate provider,
but also demonstrates why its extents cannot flow directly into an
authoritative recompiler pack.

The current NWXE SHA-256-bound experiment reports:

```text
total physical ROM              33,554,432 bytes
unique direct physical mappings  1,325,728 bytes (3.95%)
logical load images              2,066,752 bytes (overlays count by bank)
declared executable bytes        1,169,472 bytes (3.49%)
entry conclusions                    1,691 candidate + 835 supported
proven exact function owners                 0
```

The executable intervals equal the six known code sections only because the
experiment manifest supplies their boundaries. That is a target for inference,
not a discovery success claim.

**NWXE overlay regions are now recovered mechanically (2026-07-18)** — the
long-standing "overlays are absent" limitation that pinned NWXE at
36.40%/28.54%. `overlay_regions.rs` / `gate_overlay_regions` searches ROM
bytes for aligned records of the NW4E descriptor *family* (candidate
`table_offset`/`record_count`/`stride`/field-offsets whose rom_start/rom_end
fields are in-bounds, ordered, code-region-sized), then uses `delta_vote`
admissibility as the uniqueness filter that rejects spurious tables. It
re-derives NW4E's five overlays from ROM alone — WITHOUT being handed the
table at 0x539a0 (it finds 0x53988, the record base) — at 100% region
precision/recall, delta_vote mapping 5/5. On **NWXE it discovers a real
descriptor table at ROM 0x48a68 and recovers four overlay regions at 100%
precision/recall, delta_vote admitting all four deltas with zero wrong**; a
second candidate table at 0xcb058 is correctly rejected (delta_vote cannot
uniquely admit it — the discipline that killed the aligned-pointer-run
heuristic, working). The PI-DMA cross-check route is honestly reported as
non-contributing for these titles: the AKI overlay loader reads
rom_start/rom_end/vram out of a descriptor record through registers (not
`osPiStartDma` immediates), so the descriptor route is the one that recovers
the triples. Ten `gate_overlay_regions` runs are byte-identical (SHA-256
`471181f2…`). The recovered table is now wired into Phase 2 by a proof rule
that requires exactly one admitted table and exact agreement between each
record's delta-derived VA and its independently parsed descriptor destination.
`gate_d1_overlays` opens the dump only after both discovery runs finish. The
four resulting proven banks move NWXE from **36.396867% / 28.542179%** to
**49.976448% / 86.895987%**, adding 1,425 recalled functions and 2,331 total
candidates while precision rises 13.579582 points. Its stdout is
byte-identical across 10/10 runs (SHA-256
`9b0dc15f92aac10586edf98a02873c0acfc57f4ff6f00f857546fcb1ec1c4440`).

`profile_overlay_regions <ROM> [--runs N]` times normalization, exhaustive
descriptor-family enumeration, delta-vote admission, and recipe
materialization independently. Each sample prints a SHA-256 receipt over the
complete recovery plus recipes; a timing comparison is valid only when the
receipt and candidate/admission/recipe counts remain identical. The first
sample also prints each recipe's descriptor/source/load/text/data/BSS ranges
and loaded-image digest, so a later closure probe can identify the exact
unreached generation without a second inspector. The profiler does not write
ROM-derived content. A 2026-07-26 NWXE profile identified two
independent costs. First, the standalone shard workspace had left CPU-bound
host build dependencies at `opt-level = 0`; a scoped
`profile.dev.build-override` now optimizes only build scripts and their
dependencies. Second, family enumeration reread and revalidated each aligned
ROM triple for every stride and field phase. It now indexes valid adjacent
triples once, then exhaustively walks the same stride/phase combinations. A
test-only copy of the original exhaustive loop is the structural equivalence
oracle. The real-ROM receipt stayed identical to its pre-change baseline with
counts `2 candidate / 1 admitted / 4 recipes`; enumeration fell from 1,569 ms
to 14--15 ms and the complete pipeline from 1,780 ms to 201--209 ms across
10/10 receipt-identical runs.

`recomps/wm2000/scripts/wm2000-static-frontier.zsh` is the current low-cost inventory loop. It
first checks the standalone target's normal Cargo feature graph, then runs only
the dense manifest path under a sampled 2 GiB process-group cap; it does not
emit or compile generated shards and does not execute an input route. The wrapper now
requires the ROM-bound header-entry BootContext; omitting that authority used
to manufacture an avoidable `initial_bev_clear=false` diagnostic. That path emits
canonical `fn64.executable-source-frontier.v1` JSON plus its SHA-256. The
optional `FN64_WM2000_FRONTIER_BIN` names a canonical absolute prebuilt
`gate_wm2000_recompile` executable and skips Cargo while retaining the same
guard and receipt path. Without it, the wrapper performs the guarded serialized
Cargo run. The caller-attested scorecard still binds the produced receipts, not
the ambient path.
receipt binds the dense-pack digest, host bindings, every linearly decoded
`CACHE` word, direct EPI slice findings/blockers, and exact raw-PI
`lui *,0xA460` site/caller PCs. When the same three-run executable-image group
environment used by the WM build is present, the receipt also binds its
validated image identities without embedding their words. Its fixed writer
taxonomy retains indirect
PI/EPI calls, unrecognized raw-PI construction, CPU copy/decompression,
SP/SI/RDP writes, aliases, mutable descriptor state, and unadmitted exception
vectors as typed open classes. Consequently the current receipt reports
`open_frontier=true`. V1 deliberately exposes no `is_exhaustive` API: it is
fast canonical inventory and a punch-list, not a renamed 100% claim.
The same production gate now emits the separate canonical
`fn64.executable-writer-channel-denominator.v2` artifact when
`FN64_WRITER_CHANNEL_DENOMINATOR_RECEIPT` names an output path. It is bound to
the exact dense-AOT-pack digest and contains the fixed eight semantic writer
channels. All eight rows remain honestly `open`: the receipt records the
current API, coverage, and transaction blockers and cannot construct a
`complete` row until its owning validator exists.
With the retained BootContext and reproducible external-image group, the
current canonical receipt is retained outside the repository and is
reproducible by the gate. Its headline is `initial_bev_clear=true`, five open
exception vectors, 14 open
writer classes, 42 unclassified `CACHE` sites, 16 direct-DMA blockers, two
open raw-PI callers, three unclassified COP0 Status writes, four open Status
value proofs, three conditional and 52 open CPU word stores, and an incomplete
transfer inventory. The CLI prints these open-category counts directly; raw
inventory counts such as one external image or ten raw-PI primitives are not
themselves blockers.
The cache inventory now consumes the analyzer-owned CFG word class instead of
forcing every raw `CACHE` decode open: four of 46 sites are proven code and 42
remain explicitly absent from the bounded CFG classification. No absent word
is promoted to data or code from its opcode shape.
The receipt's vector denominator is the exact six destinations selected by the
current CPU model: `0x80000000`, `0x80000080`, `0x80000180`, `0xbfc00200`,
`0xbfc00280`, and `0xbfc00380`. Each entry is either bound to one exact external
image, bound to a validated machine-checkable unreachability receipt, or left
open; the unreachability form currently fails closed because no such receipt
validator exists. `0x80000100` is excluded because the current CPU model never
selects the cache-error vector. The CFG value-set engine also exposes a bounded
fixed-word-store report for ROM-word copies into watched addresses. Those
results remain explicitly conditional on source stability, so they can guide
vector provenance work without promoting initial ROM bytes into runtime facts.
The WM frontier producer now runs that pass independently over the resident
image and all four dense overlay generations. Each scan is bound to the dense
generation's name, content-addressed bank ID, ROM/load geometry, and loaded
digest; the receipt records its proven-root and reachable-block counts so an
empty finding set cannot masquerade as an exhaustive scan. Conditional stores
retain two typed open requirements -- source stability until the load and
actual execution of the store site -- while unresolved stores retain typed
data-flow blockers. This inventory watches the six modeled exception entry
words only and therefore cannot close the general CPU-writer class.
The same per-generation CFG now carries an exhaustive aligned-word inventory
of typed `MTC0`/`DMTC0` writes to COP0 Status. Each raw decode is retained as
proven code, proven data, or unclassified, and the scan also retains every open
indirect site. The canonical producer performs the raw scan; the receipt
validator requires exactly one geometry-bound scan per generation and
re-decodes every reported word. This is only the static instruction half of a
future BEV proof: captured BootContext Status, host ABI status effects, legacy
C-context copies, and new-thread context restoration remain separate runtime
authorities. Until those have a closed typed effect inventory, the three BEV
bootstrap vectors remain open even when a dense scan finds no proven-code
Status write.
Reproducible external executable-image captures now receive the same raw scan
and bounded-CFG classification. The receipt requires exactly one scan for each
external image generation and binds it to the capture's identity, range,
content digest, and first attempted fetch. Unclassified Status-shaped words or
open indirect sites remain explicit blockers; capture admission alone cannot
silently bypass the dense-image denominator.
An external capture owns an exception-vector entry only when that entry is its
reproducible first attempted fetch. Merely covering a vector with captured
bytes is not executable-entry authority and remains open.
For every proven-code Status write, the producer now reuses the existing
whole-CFG abstract interpreter and records one source-GPR proof. Finite joins
are retained, and a known-zero/known-one domain preserves bit invariants when
the complete value set is unknown. `MFC0 Status` seeds BEV as known zero after
the ROM-bound initial state has established that invariant, so the resident
read/modify/write site at `0x8002a26c` now closes without inventing an exact
Status value. Raw bytes read from load-image memory deliberately contribute no
known bits because that memory is mutable at runtime; operations after the
load must establish any retained mask. Unknown or widened values, mutable
load-image provenance, and `DMTC0 Status` remain typed blockers. The receipt
closes an individual `MTC0` when either every exhaustive finite value has BEV
clear or BEV is known zero and its only remaining blockers concern the
otherwise-open value. This reduced the measured WM Status frontier from five
open proofs to four while leaving all other headline counts unchanged.
The receipt validator now has an in-process `bev_clear_invariant` disposition
for only the three `0xbfc0...` vectors. It accepts that marker only after the
ROM-bound initial Status, every dense/external Status scan and value proof, the
exact typed 15-symbol installed host catalog, all three normal-vector owners,
and the executable-writer and transfer frontiers are closed. An opaque
schema/digest claim remains rejected. `osCreateThread` is an inductive
preservation edge (`child = caller & !FR`), not a fresh BEV source. The current
WM receipt has the real BootContext and one capture group, but it does not yet
satisfy these prerequisites because five vector entries plus its writer,
Status, store, cache, DMA, and transfer frontiers remain open.
The receipt now binds the initial Status authority explicitly. With
`FN64_BOOT_CONTEXT` absent it records `missing` and remains open; with the
variable supplied, the gate validates the canonical context against the
normalized ROM digest and header entry, fixed NTSC mode and destination code,
and the normalized ROM's IPL3 digest, then records the canonical context hash
and exact CP0 Status value. Malformed or mismatched input fails rather than
downgrading to `missing`. A BEV-clear initial value closes only that initial
state edge, not later guest writes or thread inheritance.
The production boot build, all 35 shard build scripts, and the frontier producer
now share one 15-symbol semantic discovery function. This absorbed
`__osSiDeviceBusy`, whose signature previously remained duplicated in the
shard emitter and could drift from the receipt. Each binding now records two
typed Status effects. All 15 cross the admitted legacy C adapter boundary, where the common
adapter compares Status.BEV before and after every shim invocation and traps
before full `status_reg` copy-back on any transition. The receipt therefore
classifies their current-context effect as
`c_bridge_runtime_enforced_preserves_bev`. `osCreateThread` additionally
records that the child inherits caller Status while clearing only FR; that edge
remains open until the caller's Status sources are proven BEV-clear.
The manifest producer reuses one decoded word vector for cache, direct-EPI,
and raw-PI scans. Raw-PI routine attribution retains the most recent `jr $ra`
boundary in one forward pass rather than rescanning backward for every fixed
register site, making that inventory linear in admitted image size.
Retained CPU-store scan summaries are evidence rather than open findings: a
nonempty scan with zero conditional/open findings no longer makes
`has_open_frontier` true. This does not promote bounded reachable-CFG coverage
to exhaustiveness or discharge the separate CPU copy/decompression writer
class; a verified whole-executable writer denominator is still required.
A separate future receipt may prove *successful-fetch confinement*: every
attempted fetch is resolved by the immutable digest-selected catalog, and
every executable-memory write forces exact generation revalidation before the
next instruction or traps. That is valuable evidence that a successful run is
100% AOT, but it must not be relabeled as the stronger V1 claim that all
possible executable sources have been enumerated. The stronger source receipt
remains required before the full static-recompilation goal can be called
complete.
The production boot and shard emitters also use the discovery crate's one
content-addressed dense-artifact ID function. This removes a second private
hash implementation and keeps runner registration, immutable-range admission,
and generated pack metadata on the same identity rule.
Production admission now additionally checks full 256-bit shard-source
identities. Each linked shard exports the identity of its exact source bytes
and emitted `runner.rs`; the root build independently emits the expected
source identity. Admission separately hashes the actual linked `CodeBank`
words and compares all 256 bits to the root expectation. The installed callable
identity composes the checked-in dispatch source, complete generated-pack
semantics, generated runner source, bank identity, and a typed adapter role;
direct, entry-gated, overlay-gated, instrumentation, and external-digest
callables therefore remain distinct. Dense and external runners use identity-
bearing registration, after which the existing canonical `BlockProgram`
evidence hash binds all exact words and callable identities.
The 64-bit bank ID remains routing identity only, never artifact authority.
The manifest gate no longer writes zero-valued transfer placeholders. It
retains each dense/external `ClosureResult`, reuses the same CFG already built
for Status and store analysis, and emits deterministic direct guest/host counts
plus exhaustive/bounded/open indirect sites. Exact host calls are classified
before resident range ownership; overlapping overlay targets stay ambiguous.
The scan now validates the CFG's direct-call, tail-transfer, unresolved-
indirect, and resolved-indirect denominators against the block terminators;
resolved target sets and proof states must agree exactly. It retains call
continuations for direct, branch-and-link, and indirect calls, rejects
overlapping blocks and duplicate edges, and types run-off-end, malformed delay
slots, decoder failures, traps, and reached data fences as blockers rather
than dropping them.
The analyzer result is opaque outside its module, and the source receipt now
serializes its complete evidence snapshot (coverage, direct edges, indirect
frontier, blockers, and derived counts) instead of accepting independently
caller-authored summary projections. The receipt is itself opaque and
serialize-only; arbitrary JSON cannot be deserialized directly into proof. A
future consumer that needs ingestion must use a validating loader.
The present result intentionally remains open because fact-derived roots are
not an exhaustive callable-entry denominator and `$ra` return provenance is
not yet proved. A caller assertion of exhaustive roots is diagnostic only and
also remains open. Receipt construction rejects direct arithmetic or indirect
summary/frontier count mismatches and a `complete` inventory containing
bounded/open sites.

The transfer scanner now also has the fail-closed seam needed to replace those
43 blockers with one catalog-total resolver proof. A private
`CatalogTotalTransferAuthorityV1` binds exact aligned owner/scan geometry,
owner kinds, named host targets, the six-vector denominator, and a sealed set
of resolver-policy facts. Under that authority, return, trap, and
bounded/open-indirect sites remain serialized as typed `catalog_guarded`
diagnostics while the runtime-resolved transfer inventory can become
`CatalogTotal`; the ordinary `ProvenFactRoots` path is unchanged. Production
now obtains private-field `CatalogResolverPolicyEvidenceV1` only from the
linked `fn64-cpu-runtime` implementation. It binds aligned sparse-PC admission,
exact active/source-owner lookup, the explicit thread-return boundary,
mapping/alignment faults, and the shared six-vector exception resolver to the
same build receipt and vector constant used by execution. The WM producer
validates that evidence against its exact dense/external owner and host-target
inputs, then uses the opaque authority for catalog-total scanning. Focused
tests cover the issuer, retained diagnostics, incomplete owner coverage, and
attempted authority reuse with a different catalog. The real-ROM gate compiles
on this path; a WM ROM was not available in the current environment to
regenerate its receipt.

The first canonical-install substrates now exist below that seam. In
`fn64-cpu-runtime`, `CatalogBlockProgramV1` owns the `BlockProgram`, admitted
entry, and instruction budget, captures the canonical program/runner evidence
plus the existing linked-feature receipt, and exposes only fixed-entry run,
validated entry changes, and atomic whole-program replacement. It accepts no
resolver callback. Resolver-policy evidence is implementation-issued rather
than minted by this caller-owned value; discover separately binds it to exact
catalog geometry. The separate
`HostFunctionCatalogV1` canonicalizes an exact sorted host-target/function
association, rejects duplicate or misaligned targets, and remains independent
of the legacy opaque `HostLookup`. The legacy ABI boot path still accepts
arbitrary entry/transfer callbacks independently from the program and artifact
identity and remains ineligible. The ABI's `CatalogResolverInstallV1` owns both opaque values
plus the dispatch identity and captures pointer-free program identity, entry,
budget, sorted host targets, and the existing build-feature receipt. Controlled
entry/budget/program changes refresh that evidence, while failed validation
leaves it unchanged. Static entry and transfer resolution now delegate to the
owned sparse code catalog: entry lookup requires one unique admitting bank,
transfer lookup prefers an exactly admitting source bank and otherwise also
requires uniqueness. Misalignment, sparse holes, unknown banks, and ambiguity
remain typed faults. Calls consult the exact owned host catalog first and
return the resolved function pointer in `CatalogCallResolutionV1`; they never
fall through to the legacy global host hook or discard ownership into a bare
host marker. Active physical and dynamic generations deliberately remain
outside this static resolver. Its production-AOT feature predicate is
explicitly only lane eligibility, not transfer authority.

The ABI now has that callback-free static path. A separate immutable
`CanonicalLiveBlockProgramV1` shares the consumed install across thread 0 and
spawned OSThreads; `boot_thread0_catalog_program_v1` takes no program, entry,
budget, lookup, resolver, host hook, or artifact identity outside that install.
Its concrete dispatcher follows arbitrary continuation PCs through the owned
sparse catalog, classifies calls through the exact owned host inventory, and
never consults the legacy global host lookup. Canonical evidence is available
through a dedicated snapshot only while this owner is installed. Either legacy
function or block install clears it, so compatibility callbacks cannot become
catalog-total merely by supplying similar generic block evidence. A guarded
end-to-end test executes guest call resolution, the exact catalog-owned host,
guest resume, and thread return while a forbidden legacy lookup is installed.

The canonical path now also has a closed precompiled-generation variant.
`CatalogGenerationInstallV1` pairs the resolver with a validated
`BackedPrecompiledGenerationCatalogV1` before either enters `HostState`. Every
generation owns a segmented, word-aligned mapping from its complete virtual
invalidation interval to explicit physical RDRAM offsets; spans may describe
noncontiguous page mappings, must exactly tile the interval, and cannot extend
beyond the 8 MiB device. Overlapping A/B alternatives must agree on the
VA-to-physical mapping. Digest selection streams physical bytes in virtual
order without reconstructing KSEG addresses or applying a VA mask, and its
evidence snapshot binds generation geometry, digests, shards, backing spans,
active segments, and pending physical writes.

Generation shard banks intentionally remain installed in `CodeCatalog`, but
the canonical resolver reserves them from ordinary static lookup. An active
generation wins; an inactive owned target produces a distinct activation
obligation; only a target outside the inventory may fall through to an
unclaimed static bank. Validation rejects any unclaimed static `CodeSpan`
intersecting a generation invalidation interval. Exact backing spans also
produce the executable-write denominator. A committed CPU write retires every
split active segment of the affected image before continuation resolution;
the host/DMA notification seam performs the same retirement before guest
resume. `ImageChanged`, computed transfers, calls, exception vectors, host
resumes, and spawned entries all activate by physical digest without a runtime
builder, interpreter, or callback. Guarded end-to-end tests cover a TLB-like
VA mapped to a different physical offset, catalog-owned host dispatch, A to B
replacement before B's first instruction, and host/DMA retirement of B.

This closes generation selection inside the canonical lane, not the global
writer denominator. Legacy observed-region builders remain compatibility-only
and ineligible for catalog evidence. The canonical bootstrap/import allocation
is now privately owned from typed ROM publication through HostState install;
commit checks every unreserved direct-RDRAM static bank and physical-code bank,
proves every nonzero generation-image byte belongs to at least one complete
exact catalog digest (while admitting zero/unloaded bytes), binds the matching
generation IDs, and turns its publications into journal sequence zero. The
ABI now mints a move-only completion authority only from that quiescent exact
journal state, and the writer denominator can consume it for the matching
program model. An offline discovery gate which never owns that live authority
must still report Bootstrap/Import open.
The other mutation channels remain open until every raw pointer,
mutable slice, renderer write, host ABI write, and admitted producer path is
forced through a typed journal and a model-total validator. Only then can the
sealed transfer/writer authority be minted.

The writer frontier now has a separate V2 substrate rather than relying on
V1's caller-supplied open-class vector. The canonical
`fn64.executable-writer-frontier-matrix.v2` schema requires exactly one row for
each of all 14 diagnostic classes, rejects missing/duplicate rows and unnamed
blockers, and derives the open-class projection from private state. Those 14
items are not mislabeled as mutation mechanisms: aliases, cache visibility,
vector destinations, and PI analysis gaps are separate frontier axes. The
distinct `fn64.executable-writer-channel-denominator.v2` schema fixes the
byte-producing universe at eight channels: CPU stores, PI DMA, SI DMA, SP DMA,
RSP execution/HLE writeback, RDP/renderer writes, host ABI writes, and
bootstrap/import publication. That denominator now aliases the same exact
eight-variant `WriterChannel` carried by every recompiler write event; the old
producer-less notification API no longer exists. Device-fabric DMA commits
also carry a narrower typed PI/SI/SP producer through `DmaMemory`, which the
ABI maps to the corresponding denominator channel instead of erasing it.

The canonical generation owner now seals the union of every ever-admissible
physical executable backing at first dispatch. Each attributed intersecting
write is invalidated and recorded in a hash-chained batch containing the exact
channel, declared and changed ranges, before/after digest, and retired
generation IDs. Before every later dispatch, the owner byte-compares that
sealed expectation and traps on an unjournaled change. This is live bypass
detection and attributable runtime evidence, not structural closure: the
public raw compatibility pointers, noncanonical renderer/ABI mutation paths,
and broad mutable slices still require sealed leases, checked foreign
transactions, or an opaque validator.
The canonical HostAbi, RSP, and RDP gateways now use checked ordered
transactions, but that does not prove model-total coverage. The canonical
guest running-thread mirror is likewise a checked scheduler-owned HostAbi
publication before each selected coroutine resumes; it is deliberately not
folded into the host-call lifecycle receipt, whose target/resume evidence does
not describe scheduler selection. The HostAbi denominator therefore remains
open pending a model-total validator for both boundary kinds. The canonical
bootstrap allocation and generation-image validation no longer escape between
validation and install, but an ABI receipt alone cannot complete the
Bootstrap/Import row. Only a verifier-owned selected-build writer-audit bundle
can atomically project its represented Bootstrap, SI, and SP rows into the
denominator. The bundle now also carries CPU-store authority, but the
denominator consumer has not yet admitted that fourth variant. Private class- and
channel-specific receipt variants prevent a bounded zero-finding scan or
caller-authored string from claiming completion. V1 remains unchanged and
honestly open while validators seal the present raw-pointer,
mutable-slice, renderer, and ABI mutation escape hatches and bind the exact
program/runtime model identity.

The device DMA trait is no longer one of those escapes. `DmaMemory` has a
private sealed supertrait; canonical ABI execution uses the runtime-owned,
call-scoped `ProcessDmaMemory`, whose unsafe constructor requires exact pointer
extent and a borrowed post-commit callback. It preflights full ranges, applies
the one logical-byte lane mapping, and reports the typed producer/range only
after all bytes commit. External crates cannot supply an implementation which
silently drops PI/SI/SP attribution.
`scripts/lint-writer-channel-topology.py` preserves that solved topology: it
rejects a new sealed-trait implementation, a second SI/SP device write site,
or producer erasure in the ABI's exact PI/SI/SP notification mapping. SI DMA is
therefore the closest remaining channel to structural completion. Its open
frontier is no longer producer attribution; it is model-total authority. The
canonical resolver identity binds host target PCs but not host callable
identities or writer-effect declarations, so a validator cannot yet prove
that every reachable SI initiation and device-clock advance for the admitted
program uses `ProcessDmaMemory`. The structural sweep is not such a receipt.

The first model-total prerequisite is now explicit in the ABI API. The legacy
`HostFunctionCatalogV1` install remains runnable but marks its host semantics
non-authoritative. An ABI-issued catalog accepts only `(target PC, stable shim
ID)` bindings; a private exhaustive mapping chooses the actual safe-Rust
adapter and derives its conservative writer-effect set. The move-only wrapper's
canonical receipt is included in resolver and writer-program model hashes, and
the WM canonical example uses this path for its exact 15-shim inventory. This
prevents caller-selected function pointers or effect strings from posing as
SI authority, but does not complete SI: emitted-runner semantic authority and
the SI-specific quiescent validator/denominator constructor remain open.

The WM generated graph now retains a fail-closed source attestation without
mislabeling it as that missing authority. It binds the exact checked-in root
Cargo/build/adapter sources, all shared and per-package shard sources, the
linked emitter/runtime source receipt and build features, and for every bank
the exact code digest/geometry, composite 2 KiB subrunner count, generated
source digest, and adapter role. Generic `GeneratedBankRunner` registration
produces no such projection. Even the source-attested constructor remains
non-authoritative because a separately compiled Rust function pointer has no
safe body identity and public evidence can still be paired with an arbitrary
callable. The boot-harness verifier now owns the isolated frozen build, exact
Cargo artifact selection, source remeasurement, and direct child launch. On
its fixed identity argument the selected WM child constructs this canonical
program without installing devices or executing the guest, emits exactly one
deny-unknown envelope sorted by bank, and exits. That envelope binds the
manifest/lock hashes, source-attestation fields, production feature receipt,
and each runner's exact source/code digests, geometry, composite count, and
role. It becomes a move-only build capability only inside the verifier which
selected and launched that binary; direct child output remains
non-authoritative. The boot harness now consumes that build capability in a
fixed Bootstrap audit child mode immediately after canonical boot, before any
guest scheduling setup. The child consumes the ABI's move-only sequence-zero
completion receipt and emits one nonce-bound deny-unknown projection. The
parent revalidates its retained build around each bounded launch and requires
ten distinct nonces with identical nonce-excluded semantics, binding the
selected build to the ROM, writer-program model, resolver, generation catalog,
bootstrap receipt, journal root, and watched bytes. Public reports and copied
series evidence remain non-authoritative. No private Bootstrap series has yet
been run, so this addition does not close the production denominator row.

The harness now has a one-build writer-audit session for the expensive live
feedback loop. One retained `VerifiedGeneratedRunnerBuildV1` can independently
run Bootstrap, CPU-store, SI, and SP exact-ten series once each and seal any completed
subset into a move-only bundle. A failed channel stores no partial success and
does not erase earlier channel evidence; duplicate success is rejected. The
bundle hash binds its exact completion bitmap, common build/binary/private-input
identity, nested series authorities, and cross-channel program identity/model.
It exposes evidence only, never the selected path or private inputs, and is not
itself denominator completion authority. The existing consume-one-build SI/SP
APIs remain compatibility wrappers over the same borrowed-build internals.

CPU instruction stores now have the same selected-build outer authority path.
The fixed child arms the ABI's move-only CPU trace epoch after canonical boot
and immediately before guest scheduling, so bootstrap and host setup stores
cannot satisfy the audit. It retries only the explicit no-store frontier,
then consumes a quiescent ABI receipt containing at least one typed post-commit
store and emits one nonce-bound, deny-unknown line. The parent independently
recomputes that receipt, revalidates the retained selected binary and private
inputs around every bounded launch, and requires ten distinct challenges with
identical nonce-excluded semantics. The move-only series binds the common
build, binary, private inputs, build/program identity, program model, resolver,
host catalog, sealed journal/watched state, and exact CPU-store trace digest.
Copied report or series evidence remains non-authoritative. No private CPU
series has been run, so this mechanism does not yet close the production row.

The boot harness also consumes that build capability in a fixed SI audit child
mode and an exact-ten parent-owned series. Fresh distinct
nonces, retained staged private inputs, binary/input revalidation around each
watchdog-bounded launch, one content-silent report line, and identical
nonce-excluded semantics are all required before a move-only series capability
is minted. That capability binds the selected build plus program-model,
resolver, host-catalog, journal, watched-state, and SI-transition identities.
No private exact-ten series has yet been run, so the current runtime SI row
remains open. `WriterChannelDenominatorV2::complete_si` still consumes only
that move-only series capability, revalidates its authority digest, requires
an exact canonical writer-program-model match and an open SI row, then retains
a private SI receipt bound to the series authority. The verifier-owned bundle
is the atomic selected-build path when it represents SI alongside Bootstrap
and/or SP. Public report or series evidence has no completion API. The ABI-local
half of the path remains
intentionally insufficient: its private validator can mint one move-only,
non-serializable runtime-state prerequisite only from the production-AOT
canonical owner with an ABI-issued host catalog, a balanced retained SI
transition stream containing a PIF-to-RDRAM commit, no pending SI/device or
writer transaction, and live watched bytes matching the sealed journal. The
receipt binds the exact program model, resolver, host catalog, final journal
root, watched digest, and SI transition digest. It does not bind a
verifier-selected executable and cannot be passed to `complete_si`; only the
outer series which owns the selected build can close the row. The fixed
selected child begins a fresh retained-device-trace window before
the bounded audit so an earlier SI transaction cannot satisfy the minimum;
the ABI validator rejects non-monotonic `(cycle, sequence)` order. Its evidence
also counts executable-journal declarations attributed to SI, but permits zero:
the journal clips declarations to executable backing, while normal 64-byte PIF
controller buffers are data. That observation count is not substituted for
the build-owned structural proof.

SP DMA now has an ABI-local prerequisite of the same deliberately insufficient
authority class. A move-only begin-epoch token is minted only while canonical
bootstrap/journal/device/ABI SP state is quiescent; minting atomically clears
retained device history and re-enables retention. Taking against that exact
program-bound epoch requires a balanced double-buffered SP trace, exact queued
handoff before the single terminal busy-clear, and at least one
RSP-to-RDRAM commit. It binds the sealed journal and watched bytes but carries
no generated-build authority and grants no denominator credit. Raw SP DMA
does not raise an MI interrupt or publish an OS notification, so the validator
uses the public SP DMA busy lifecycle rather than fabricating a notification
stage. The selected WM runner now has a fixed SP child protocol which arms the
typed epoch immediately before bounded canonical guest/device scheduling. It
accepts only a real admitted-guest RSP-to-RDRAM lifecycle after all SP
device/task/ABI work drains; transient pending/no-transition/no-writeback
states retry, while invalid ordering and other invariant failures trap. The
child consumes the ABI receipt once and emits one nonce-bound deny-unknown
line. The boot-harness parent consumes the selected build and shares SI's
environment-cleared staged-input launcher, bounded output/watchdog, and
pre/post integrity validation. Ten distinct OS-random nonces with identical
nonce-excluded semantics mint a move-only SP-series prerequisite. No private
SP series has been run, so the production SP row remains open. The writer
denominator's `complete_sp` API still consumes only that move-only series
capability, revalidates its authority digest, requires an exact canonical
writer-program-model match and an open SP row, then retains a private SP
receipt bound to the series authority. The selected-build bundle provides the
atomic combined path without making copied reports, copied series evidence, or
the ABI-local prerequisite admissible. The SI prerequisite retains its
documented selected-child epoch
obligation until its separate outer verifier is migrated; these two freshness
boundaries are intentionally explicit.

The canonical renderer boundary now snapshots only the sealed executable
backing union around synchronous live task execution and around each validated
shadow-image publication. It diffs in logical physical-byte order and emits
coalesced `RdpRenderer` declarations before guest resumption. Framebuffer-only
writes outside executable ownership do not allocate journal entries. This
closes producer identity for the enumerated production renderer entry points;
the renderer trait's broad mutable slice remains a structural escape for
noncanonical callers and therefore does not yet mint the channel receipt.

Every exact catalog-owned host call is now a per-guest-thread LIFO parent
mutation transaction. The owner commits its current `HostAbi` prefix before a
coroutine suspends and before each synchronous RSP/RDP child enters, commits
the child batch immediately, keeps the parent open across a yield, and commits
the residual `HostAbi` suffix on return. This preserves
`HostAbi -> child/device -> HostAbi` order even when all writers change the
same executable byte, without pretending the broad raw pointer itself is
safe. Open transaction frames and pending writes are live quiescence
diagnostics; the V1 journal root binds committed batches, not those in-flight
frames, so it is not a completion receipt. Compatibility/noncanonical pointer
and mutable-slice escapes, exact model binding, and a validator-owned
completion constructor remain open.

Generated-crate build profiling established a separate bottleneck. A debug
shard takes about 20 seconds of rustc codegen and peaks near 2.6 GiB; changing
the dense subrunner from 4 KiB to 2 KiB and moving its duplicate word literals
to a binary include did not improve those measurements, so neither experiment
was retained. Historical guarded tests measured `-j2` at 4.24--5.16 GiB and a
shard-only `-j3` batch at 5.28 GiB, but a later unbounded session exhausted
system memory. Those throughput results are not the current workstation
safety setting: Cargo/rustc work uses one job under an aggregate process-group
guard. The production full-graph measurement used a 4 GiB ceiling and a 40%
system-free floor after a 2 GiB ceiling stopped one source-heavy shard at
2.079 GiB aggregate while the system still reported 62% free. Parallel shard
compilation belongs in a future isolated
build-host workflow; release codegen remains outside the measured safe
envelope. The guard disables zsh's automatic background-job niceness so
monitoring does not lower the launched build's scheduling priority. It now
also records the largest child PID, resident set, and command at the aggregate
peak and terminating sample, so a failed experiment distinguishes generated
`rustc` pressure from the final linker.

The safe local build entry point is now
`scripts/guarded-cargo-build.zsh`: every invocation names `check`, one-package
`build`, or `full` mode and an explicit in-repository manifest. It fixes Cargo
at one job and defaults to the measured 4 GiB aggregate / 40%-free envelope.
The underlying guard retains its historical defaults for existing direct
callers, but now launches a dedicated macOS session/process group and monitors
that exact PGID until it is empty. A child remains owned after its original
parent exits or it is reparented; threshold termination signals only that
group. Process-table, free-memory, wall-clock, and JSONL failures fail closed.
It can additionally enforce an opt-in wall-time ceiling and append path-free
JSONL samples containing only elapsed time, aggregate/peak RSS,
largest-child RSS, free-memory percentage, and a controlled terminal reason.
Neither profiler records argv or ROM paths. RSS and system-free enforcement
are one-second samples, not kernel hard limits: a process can overshoot between
samples before group termination. This guard does not install an OS resource
limit. `scripts/test-memory-guard.zsh` covers command-status propagation,
leader exit plus a reparented survivor, TERM-to-KILL escalation, and path-free
JSONL; the guard hardening passed 10/10 consecutive shell-only runs.

The guarded build changes compiler partitioning without changing guest
coverage or bank identity. Each existing 4 KiB callable subrunner is emitted
as its own non-inlined Rust module. An isolated opt-level-0 hot shard completed
at 1.863 GiB, but the final root link immediately exceeded 2 GiB while loading
the 34 large unoptimized artifacts. An opt-level-1/64-CGU attempt also crossed
the guard because too many LLVM partitions remained live concurrently. The
next measured configuration retains 16 codegen units and divides the existing
64 KiB artifact into 32 separately non-inlined 2 KiB modules. This combination
was not covered by the earlier 2 KiB experiment, which left all functions in
one module. The isolated hot shard completed in 2m22s including dependency
rebuilds at 1.991 GiB peak RSS; its rlib fell from 33 MiB at opt-level 0 to
5.7 MiB. The complete 34-artifact production build and root link then finished
under the 4 GiB/40%-free guard in 39m48s. Aggregate RSS peaked at 2.117 GiB,
the largest `rustc` at 2.053 GiB, and system-free memory remained 65--66% near
completion. The linked binary was 111 MiB with 105.6 MiB of text. The source-
heavy shard that crossed the earlier 2 GiB guard completed without raising the
2.117 GiB peak, so the failure was a narrow ceiling rather than cumulative
memory growth. Splitting each logical 64 KiB artifact across two 32 KiB backing
crates remains a viable lower-memory mechanism, but the measured bounded build
does not justify that additional wrapper graph yet. Smaller native artifacts
also reduced link pressure. The
standalone dev profile disables debug info and incremental state
while retaining debug assertions; the old target had grown to roughly 100 GiB,
about 68 GiB of it incremental state. This configuration is not a fix until a
shard and full link both complete under the guard; both now do.

Generated-source structure is now inspectable without Cargo or ROM access via
`tools/profile_generated_shards.py TARGET_DIR`. It selects the newest
`runner.rs` per package and reports source bytes/lines, repeated verification
and post-step boilerplate, `finish!` invocations, retained runner generations,
and shard rlib footprint. `scripts/cargo-target-inventory.zsh TARGET_DIR...`
separately reports target/build/deps/incremental sizes and the most duplicated
build-output package generations. Both tools are strictly read-only and have
no clean or prune operation; deleting a target remains an explicit human
decision. This matters because disabling incremental compilation prevents new
state but does not remove historical profile/feature generations already on
disk.

The first source-compaction mechanism moves the ordered straight-instruction
boundary decision into `fn64-cpu-runtime::post_straight_instruction_exit`.
Every generated straight arm retains its architectural operation, CP0 Random
advance, retirement count, and transfer; only the repeated
executable-write-before-checkpoint choice is shared. The source profiler
counts this helper call as post-step boilerplate, so a reduced percentage
cannot come from hiding the replacement line from the metric. The helper is
non-inlined to test reduced LLVM work, which adds a host call per ordinary
guest instruction. This remains an experiment until one identical worst shard
has before/after source, compile time, peak RSS, and rlib evidence plus an
unchanged runtime oracle and measured execution cost; source reduction by
itself is not a retained performance claim.

The second compaction keeps live executable-image verification in every dense
AOT turn but removes its per-arm source expansion. Each 2 KiB subrunner now
owns one expected-word table and performs the current-PC verification once at
the top of its local dispatch loop; architectural delay words remain verified
at the control-transfer site, including taken-only branch-likely behavior and
the single affine shard-edge lookahead. On the historical worst standalone
shard this reduced generated source from 19,064,258 to 14,799,491 bytes
(22.4%), the guarded cold graph-plus-shard run from the documented 142 seconds
to 131 seconds, and sampled peak RSS from 1,991 to 1,216 MiB. The complete
25-test bank-runner gate and ten N64Recomp-C codegen-oracle tests passed in an
isolated feature-clean target. A pre-change 200,000-step WM route baseline was
116.85 seconds; a post-change full linked runner and route measurement remain
required before claiming unchanged runtime cost. The cold full-audit build is
therefore the next compile-side gate, not an assumed extrapolation from source
size alone.

That cold gate generated 468,340,637 bytes across all 35 runner sources and
compiled all 35 shard libraries under the fixed 2,048 MiB/40%-free envelope;
sampled peak process-tree RSS was 1,095 MiB. It did not mint a build receipt:
the identity child exposed a pre-existing protocol drift where the runtime
issuer hashed the V2 source-binding domain and the independent verifier still
hashed V1. Both now consume the runtime's exported V2 domain constant while
retaining independent ordered-field reconstruction. A new cold run remains
required; the failed identity validation is evidence for build feasibility and
the caught verifier bug, not writer-channel completion.

The third compaction moves aligned-address and checked-memory failure
continuations into `fn64_cpu_runtime::generated_support`. Its typed
`ArchitecturalFaultSite` constructors make straight versus delay-slot EPC/BD
state explicit; `finish_data_access_error` retains the distinct guest-
exception and host-admission retirement rules and still passes through the
executable-write finalizer. The successful path remains inline and calls no
helper. A source-shape sweep rejects reintroduced inline exception conversion.
The complete 26-test bank-runner gate passed 10/10 guarded runs. On the same
historically worst shard, source fell from 14,799,491 to 11,651,362 bytes
(21.3%), cold graph-plus-shard time from 131 to 107 seconds (18.3%), and rlib
size from 27,971,416 to 22,327,176 bytes (20.2%); sampled peak RSS was unchanged
at 1,217 MiB.

Raw fixed-chunk deduplication remains rejected: the complete prepared tree had
no repeated 2 KiB chunks. `inventory_dense_body_reuse` instead counts exact
maximal straight bodies and control/delay pairs without exposing words,
addresses, paths, or source. On the worst 64 KiB artifact, the current 2 KiB
boundary had 17,391 unique semantic slots out of 18,419, while artifact-wide
sharing had 17,073. The resulting upper bounds are only 5.6% and 7.3%; that
does not clear the 15% gate for changing execution shape. Cross-artifact body
sharing is deferred because it would introduce a shared generated-crate graph
and may erase the incremental and memory isolation gained from sharding.

The shard build profiler is observational. `FN64_PROFILE_BUILD` is sampled
only when a build script already reruns for a real input change; toggling it is
deliberately not a Cargo invalidator. Generated shard and top-level pack files
are written only when their content changes. After the fix, toggling the flag
on the full 34-shard graph completed hot in 0.12 seconds without recompilation;
the one-time guarded rebuild completed in 51.5 seconds and peaked below 3.83
GiB aggregate RSS.

`recomps/wm2000/scripts/profile-wm2000-shard.zsh` is the bounded compile-side counterpart.
It defaults to the historically worst
`wm2000-block-overlay-2-shard-04`, creates a fresh explicit target without
deleting any existing target, fixes Cargo to one job, and runs under the common
memory guard. Its path-free JSON binds total cold-graph wall time, sampled peak
RSS/final free memory, generated source bytes/lines and `finish!` count,
exact-body reuse totals at the current 2 KiB and candidate 64 KiB scopes, and
the selected shard rlib count/bytes. Cargo does not expose a stable boundary
that prebuilds this standalone workspace's exact dependency units without also
building the selected shard, so the total is labeled
`cold_dependency_graph_plus_shard`; the existing build-script phase timings
separately retain normalization, generation, extraction, binding, emission,
and write costs. Child output is sanitized against the exact ROM pathname.
The fresh target, sanitized log, guard samples, and summary are retained for
explicit inspection; the script never cleans historical targets. `--dry-run`
and `--selftest` exercise selection and redaction without invoking Cargo.

The static-frontier/current-scorecard host compile defaults to line-table debug
info. Full debuginfo crossed the fixed 2 GiB guard at a 2,160 MiB sample,
whereas the identical producer completed at 1,414 MiB with
`CARGO_PROFILE_DEV_DEBUG=1`. Generated guest-code profiles and selected-build
identity remain independently fixed.

The next authoritative fresh selected-build attempt remained serial and hit
its wall-time ceiling at exactly 2,400 seconds. It did not complete build
selection, so it produced no verified-build, writer-audit, or scorecard
receipt. No generated-source byte count or completed-shard count is attributed
to that attempt: retained prepared trees with such counts predate it and are
not evidence about the timed-out build. An explicitly non-authoritative full
root `cargo build -j2` experiment over the same generated graph then completed
cleanly in 19m07s. That establishes two-way Cargo parallelism as an interim
build unlock, but 19 minutes is still too slow for the intended ROM feedback
loop. Admitting parallelism to selected-build authority requires a versioned,
explicitly bound job-count contract; the observational run alone grants no
writer or scorecard authority.

The next compiler-architecture experiment cleared its representation gate.
`static-micro-op.v1` stores each admitted word as one canonical eight-byte
record and keeps bank-qualified span geometry in a small binary envelope. On
the same complete 35-package WM inventory it produced 516,688 records in
4,135,884 bytes, below the fixed 12 MiB ceiling and 98.84% smaller than the
355,651,449 bytes of generated Rust measured after the fault-helper
compaction. The direct content-silent profiler completed in 5.54 seconds and
then returned the same count, size, and inventory digest in ten consecutive
real-ROM runs. It retains raw expected words and exhaustive decoder-derived
opcode/flag validation. The explicitly non-production executor now sends every
admitted non-control raw word through the shared lane-neutral semantic kernel;
BEQ/BEQL remain the deliberately narrow local control-pair slice. Its emitted-
dense-runner differential compares exact exit, complete CPU evidence, full
RDRAM, and ordered MMIO effects, including direct MMIO and mapped/TLB data
aliases. It covers executable writes, ERET, COP1, prior-retirement faults,
arbitrary interior entry, live primary/delay mismatches, likely annulment,
checkpoints, retirement accounting, COP0 Random, and straight/delay RI EPC/BD,
and passed 10/10 consecutive guarded runs. Because both sides intentionally
share semantic/runtime helpers, this is integration equivalence rather than an
independent ISA oracle. Direct-RDRAM instruction verification, remaining
control families, host-call transfer boundaries, and an independent format
oracle still prevent production promotion. Source receipt V3 adds the missing
`fpu.rs` edge while preserving V1/V2 evidence.

The first real-WM admission rerun exposed two separate shapes. Control-shaped
delay words are valid artifact members because the same word may be entered
directly; admission now preserves them, while actually consuming one as a delay
returns the experimental lane's loud unsupported fault. The rerun then stopped
at the exact 64 KiB end of `wm2000-block-overlay-3-shard-03`: branch
`0x80121b8c` requires the dense emitter's affine lookahead word at
`0x80121b90`. `static-micro-op.v1` cannot mark a record delay-only, so adding it
would falsely admit a new direct entry. V2 now encodes an explicit optional
per-span lookahead after the owned records. Admission permits it only for the
final owned control when no owned delay exists; execution can consume and
live-verify it only through that control. Owned-PC resolution and instruction
counts never include it. The complete V2 WM profile now admits all 35 packages:
516,688 owned instructions, 4,135,951 bytes, profile schema
`fn64.wm-static-micro-op-profile.v2`, and one canonical inventory digest. Ten
consecutive real-ROM profiles returned that exact schema, package count,
owned-instruction count, byte count, and digest.

Runtime profiling is separately opt-in through `FN64_PROFILE_AOT_BANKS`. A
200,000-step resident probe attributes 56.5% of AOT entries to shard 00, 34.1%
to shard 03, and 9.4% to shard 02. A historical experiment scoped dev
`opt-level = 1` to the first two hot packages and reduced their
identical-priority probe from 24.75 to 18.41 seconds. The current complete
standalone graph instead uses opt-level 1 / 16 codegen units for all dependency
packages because the final link could not safely load the unoptimized shard
catalog; only the handwritten reference renderer, runtime, and ABI rise to
opt-level 2. Removing the guard's unintended `nice(5)` lowered the historical
final probe to 9.07 seconds. Diagnostic execution and host-boundary histories
remain complete by default but the exploratory harness suppresses them unless
their trace outputs are requested, holding the measured process at 161 MiB
instead of allowing tens of millions of observations to grow without bound.

**Overlay recovery now crosses engine families (2026-07-18).** The AKI search
was physical-offset-only, so `gate_overlay_generalize` first found zero tables
on OoT/GoldenEye/Perfect Dark — a diagnosed VROM-addressing shape gap, not a
logic gap. `file_table.rs` closes it: it mechanically recovers a ROM's file
table (dmadata-shape `(vrom_start, vrom_end, rom_start, rom_end)` records —
ordered contiguous VROM, in-bounds physical backing, identity record required,
admit on uniqueness), giving a VROM→physical translation, and
`overlay_regions.rs` then runs the descriptor-family search over VROM-located
tables resolved through it. On OoT it recovers the file table at physical
`0x7430` (matching `dmadata`) and **414 overlay regions at 100% precision /
88.5% recall** (actor and Kaleido descriptor tables admitted; the physical
`dmadata` too), all held-out — the dump opens only after recovery. SM64 (a
single static image) correctly admits **zero overlay tables** — the
negative control holds, no hallucination. GoldenEye and Perfect Dark recover
nothing and are reported ungraded (no vendored key). The AKI physical path is
untouched: `gate_overlay_regions` (`471181f2…`) and `gate_d1_overlays`
(`9b0dc15f…`) are byte-identical. Honest open frontier: OoT's effect and
gamestate descriptor families are enumerated but yield fewer than the
two-region admission floor, so they stay open rather than force-promoted.
VROM materialization is allocation-bounded at the decoder boundary: the
default automatic path admits at most 64 MiB per complete decoded file, while
explicit-limits entry points may lower that ceiling. A tiny Yaz0 stream whose
header or recovered VROM extent declares a larger output remains unavailable
before any output reservation and therefore cannot mint a descriptor table or
bank mapping. Distinct capped files are typed recovery diagnostics and their
count is emitted in the automatic strategy outcome, keeping this resource
frontier visible in producer manifests.
`gate_overlay_generalize` is 10/10 byte-identical with the full OoT+GE+PD+SM64
set (SHA-256 `5401e638…`).

**End-to-end payoff — OoT graded with mechanically-recovered overlays**
(`gate_d1_oot_overlays`, held-out). OoT's
existing 99.567%/72.331% grade uses hand-supplied `oot_load_image_tables`
geometry (a per-game input the engine did not infer). Running the identical
function-entry grade through the *mechanically recovered* overlays instead
answers whether automation can replace that hand geometry. The three-way
result: (A) boot-only 62.500%/0.823%; (B) mechanically recovered
**99.567%/72.331%**; (C) hand geometry 99.567%/72.331%. Before the held-out
dump is opened, B and C must have the exact same canonical scoped candidate
receipt: combined and per-detector physical identities, physical call-source
provenance, and every ungradable bank-qualified identity. The gate prints its
SHA-256 and then independently requires equal combined/per-detector grade
aggregates and ungradable counts. **Mechanically-recovered overlays therefore
reach the hand-supplied geometry ceiling by exact candidate identity, not just
by offsetting precision/recall totals.** The comparison is explicitly
bank-scoped: full discovery also retains
the resident `code` load image recovered from the boot-time
`DmaMgr_RequestSync` operands, while that independent image contributes to
neither B nor C. Exact overlay geometries repeated by overlapping admitted
descriptor runs mint one deterministic bank identity, so the full result is
one boot bank, 468 overlay banks, and one resident-code bank.** This closed in
three steps: descriptor-corroborated actor mapping
(each open record's own `vram_dest` field admitted only if a CFG rooted there
reaches valid in-window code and the VA is unique) took B from 48.450% to
69.449% recall (actor 167→412 of 426 sub-banks, 0 wrong); then sound
below-floor admission (a table below the two-region floor is admitted iff its
single record is descriptor-corroborated AND VA-unique) closed the last 14
actor sub-banks plus the effect (36/36) and gamestate (4/4) tables. Final:
actor 426/426, effect 36/36, gamestate 4/4, kaleido 2/2 — all 468 overlay
regions recovered, 0 wrong, 0 missed. The AKI physical path never fires the
corroboration or below-floor rules (their tables map fully via delta_vote), so
`gate_overlay_regions` (`471181f2…`) and `gate_d1_overlays` (`9b0dc15f…`) are
byte-identical throughout. `gate_overlay_generalize` is 10/10 byte-identical
(`dec5742e…`). **This proves the
"port any N64 ROM without per-game hand geometry" thesis for the four overlay
descriptor families: automation matches the hand-encoded overlay tables, not
approximately but exactly.** It is not yet proven for the ROM as a whole:
resident `code` now has a mechanically recovered load mapping, but its precise
executable intervals are still open and `n64dd` still lacks a recovered VRAM
mapping. Those images remain outside the overlay-only 72.33% figure, and
`gate_closure` still feeds hand-supplied
`oot_reference::oot_load_image_tables()` rather than the mechanically
recovered set.

**Retained historical execution-closure scoreboard (`gate_closure`, held-out,
10/10 byte-identical `4ff3a44c…`).** This is the last retained all-corpus
baseline, not current-worktree evidence. Snapshot schemas v2 and v3 changed
block and source authority after this run. A current WM regeneration is
recorded below; the other corpus counts remain historical. In that retained
run, every reachable CPU
transfer destination was classified `exact_aot` (inside a proven exact
owner) / `block_aot` (proven reachable code, no source-level owner claimed) /
`dynamic_mips` (open/bounded indirect the interpreter fallback covers) /
`unsupported` (lands outside every known mapping — the release-blocker). Per
ROM (destinations, measured at retained gate digest `1c6db903…`): NW4E block_aot
22,215 / dynamic_mips 732 / **unsupported 11**; NWXE exact_aot 349 / block_aot
17,642 / dynamic_mips 1,898 / **unsupported 20**; OoT (whole ROM: the resident
boot bank plus 468 composed VROM overlay banks) exact_aot 2,693 / block_aot
14,683 / dynamic_mips 11,829 / **unsupported 568**.
Here `dynamic_mips` means executable by the implemented `dev-interpreter`
lane. The production pure-AOT feature graph excludes that lane and instead
requires static catalog admission.

The retained `unsupported 20` is not an address-level artifact: the old gate
printed only a bounded VA list, and neither that list nor incoming CFG edges
survived in the repository. It also classified concrete targets solely against
the union of proven `RomMapping` VA intervals. It did not consult the exact WM
host catalog, the six modeled exception-vector image authorities, the canonical
resident/overlay generation catalog, or runtime TLB/KSEG alias state. Therefore
the aggregate cannot honestly distinguish a missing load mapping from a host
call, vector entry, or runtime mapping, and must not be reverse-engineered into
twenty invented addresses. `FN64_CLOSURE_AUDIT_DIR` now opts the gate into a
schema-tagged JSON artifact that retains every unsupported VA, every incoming
bank/block/source-site edge, all composed bank byte identities and mapping
geometry, the scoreboard, and an explicit list of authorities not consulted.
Its SHA-256 is printed; it is diagnostic and cannot be loaded as execution
authority.

The historical scorer also omitted direct-call continuations, branch and
branch-likely fallthroughs, resolved/open indirect-call continuations, and
ordinary `Fallthrough` successors. Current `closure` measurement has one typed
successor enumerator shared by `scoreboard`, `classified_destinations`, and the
unsupported audit. It retains those edges plus every taken/tail/resolved target
while preserving destination-VA deduplication for headline counts. End-of-bank
fallthrough and call-return regressions prove that a concrete successor just
outside the mapping is no longer silently absent. This edge-denominator change
is another reason the retained 20 cannot be attributed to current HEAD before
a ROM-bearing regeneration.

Snapshot schema v3 and execution-closure-audit v3 now bind the source side of
that denominator to the authority-rooted CFG already used for cross-bank proof.
The broader CFG seeded by candidate and supported traversal hints remains in
the snapshot for discovery coverage, but its blocks and indirect sites cannot
manufacture execution-closure successors. A synthetic candidate-only far call
is excluded while the byte-identical transfer from an authoritative root and a
transfer reached through an authoritative non-owner block remain counted. The
v3 audit additionally retains every concrete `dynamic_mips` destination with
its authoritative incoming edges and sanitized block/owner blocker kinds, plus
every bounded/open indirect site's typed resolution record. It retains proof
metadata and addresses, never ROM words or bytes. The current caller-attested
WM regeneration predates that diagnostic addition: its closure-audit v2
measures 1,823 authoritative destinations: 1,773 `block_aot`, 40
`proven_code_no_owner`, 10 `open_indirect_site`, and **0 unsupported**. Concrete
destination bytes are 97.793712% AOT. The private closure receipt retains its
canonical digest alongside the scorecard.
This corrected denominator is not directly comparable to the historical
19,909-destination count: candidate traversal edges were removed and modeled
continuations/fallthroughs were added. Reusing those same composed authority
closures in the source-frontier scan corrected a second denominator error: the
earlier boot-entry-only scan reported 274 direct transfers, one closed indirect,
and zero blockers as complete. The aligned five-bank scan instead reports 2,800
direct transfers, 12 closed indirect sites, and three direct targets ambiguous
across overlapping overlay-generation owners. Its transfer inventory is
therefore honestly open. Executable-memory sources and all eight writer channels
also remain open, so the aggregate makes no completion claim.

The authority-aware successor already exists in the WM source-frontier path:
`transfer_scan` classifies exact installed host calls before guest ownership,
and accepts catalog-guarded returns, traps/vectors, and dynamic transfers only
under an opaque catalog-total authority bound to the
exact owner and host-target inventory plus implementation-issued resolver
policy. Its complete evidence is serialized into the source-frontier receipt.
Wrapping that scan in a second "closure v2" counter would lose edge semantics
and duplicate policy, so the next ROM run must regenerate that receipt and the
new diagnostic artifact rather than promoting the historical 20 through a new
caller-authored address vector.

**Retraction.** An earlier revision of this paragraph reported OoT `unsupported
8` and headlined "**6–20 per ROM**". That was measured before whole-ROM VROM
composition (1 composed bank, not 923); that retained post-composition run
measured OoT at **568**, all of them `outside_all_mappings` — zero land in
proven data. The "6–20 per ROM"
headline does not survive and is withdrawn. NW4E (11) and NWXE (20) were
unchanged in that historical transition; none of these counts describes
current HEAD until the gate is regenerated.

**What `dynamic_mips` actually is.** Splitting the `proven_code_no_owner`
label out of `mapped_not_proven_code` showed the old bucket was 96–99%
mislabelled: NW4E 632/650, NWXE 1,756/1,833, OoT 11,386/11,549 are words the
CFG had **already proven are code**, which block proof then declined to admit.
They were being reported as if discovery could not decode them. This is a
block-admission problem, not a decoding or recall problem.

The `block_proof_blockers` histogram — the first such measurement recorded for
these ROMs — says which refusal dominates, and it differs by ROM:

| ROM | top blocker | second |
|---|---|---|
| NW4E | `ambiguous_owners` 7,094 | `entry_not_authoritative` 2,068 |
| NWXE | `ambiguous_owners` 6,146 | `entry_not_authoritative` 4,303 |
| OoT | `entry_not_authoritative` 59,488 | `ambiguous_owners` 3,358 |

One block can carry several blockers, so these exceed the refused-block count;
they rank causes, they do not count blocks. For OoT the single largest lever on
AOT coverage is entry authority; for the two AKI titles it is owner ambiguity.

Those histogram values describe the retained schema-v1 block proof. Since
schema v3, including the current schema-v6 snapshot, competing function owners
are no longer treated as an
executable-byte blocker: a shared block is admitted when at least one claimant
root is independently authoritative and the existing code, terminator, and
unique-ROM-backing proofs all pass. Its evidence carries every authoritative
reachability root in canonical order. That mechanism additionally projects exact
claimant roots from the separately partitioned authority-only CFG onto a broad
block only when one authority block fully contains it without splitting a
control instruction from its delay slot. In projection mode broad-owner roots
are never unioned back in. This recovers block proof hidden by candidate-root
partition splits without promoting candidate owners.

**Outcome 8 (current, caller-attested; private artifacts).** The latest WM
regeneration measured **1,823 unique destination VAs**: 1,810 `block_aot`
(7,240 bytes), 13 `dynamic_mips` (12 bytes), and **0 `unsupported`**. The
dynamic entries separate into 3 concrete destinations and 10 indirect sites;
the concrete VAs are `0x8010211c`, `0x8013b744`, and `0x8013c3c0`.

Relative to outcome 6, the denominator contracted by 515 destinations, AOT by
503 destinations / 2,012 bytes, and dynamic by 12 destinations. This comes
from removing unsound authority for calls whose target VA matches several
overlay generations; the dynamic reduction is a denominator artifact, not 12
coverage wins. Outcome 6's direct sources comprised boot (2,800), overlay 2
(552), and overlay 3 (215), while outcome 8 retains only the boot source
closure. The six previously unique targets are therefore absent, not closed.
Of the prior concrete dynamic set, 13 destinations disappeared with those
source closures, `0x8010211c` remains, and `0x8013b744` plus `0x8013c3c0` newly
appear as dynamic. This improves soundness but regresses admitted coverage
pending typed activation-compatibility authority.

The sibling transfer inventory is now 2,800 entries: 2,660 guest targets, 137
installed hosts, and 3 open targets, with 12 indirect sites closed. The source
receipt differs from outcome 6 because its authority-derived inventory
changed; the writer receipt remains byte-identical. These are
destination-VA and transfer-inventory diagnostics, not ROM-byte coverage,
code-byte coverage, path coverage, or proof that all generation combinations
are closed. In particular, overlapping generation activation still requires
runtime/catalog authority. The outcome-8 receipts remain in a caller-owned
private artifact directory outside git and carry the scorecard's `current` /
caller-attested label; existing schemas do not bind the worktree, so this
measurement is not verifier-owned authority.

The reusable `generation_topology` substrate now derives the catalog-shaped
geometry needed to replace VA fanout: a ROM-bound immutable boot prefix,
displaced resident-tail generation, and every exact overlay image and
invalidation interval. Dense-manifest and independently admitted recipe fields
must agree exactly. From the initially resident tail it enumerates, under an
explicit state cap, every canonical segment arrangement permitted by applying
the split/invalidate geometry. These are not runtime-reachable states: the
runtime begins with no active segment, and activation requires modeled bytes,
writes, and digest selection. The bank-qualified coexistence query is therefore
only a negative filter; `false` rules an edge out while `true` proves nothing
about execution. An overlapping address alone proves nothing. The serialized
`topology_sha256` is explicitly diagnostic-derived, not the backed runtime
catalog's canonical-definition digest. This remains a topology substrate until
composition binds its materialized banks, digests, and generation identities
to that actual runtime catalog.

The first such binding is now implemented for exact direct transfers. Its
move-only capability binds normalized-ROM, dense-manifest, topology, backed
catalog-definition, source generation/bank/site, transfer kind, destination,
and selected target-generation identities. Composition rejects cross-topology,
cross-catalog, or wrong-generation reuse before its fixed point. Exclusion is
limited to exact physical-byte conflicts protected by complete source
invalidation backing, and the exact control/delay pair must be proven not to
write catalog-backed memory. Calls add callable authority; jumps add only
reachability. Zero compatible generations is a typed activation miss and
multiple compatible generations remain ambiguous. Neither geometry-only state
enumeration nor a runtime observation mints this capability.

The bounded fixed-point driver is now wired into `gate_wm2000_recompile`. It
starts from ordinary validated composition, scans only authority-reachable
direct calls and jumps whose target VA has multiple prepared owners, validates
each request against the real backed catalog, and recomposes after newly
authorized capabilities. Findings are deterministically ordered and remain
typed as authorized, activation miss, ambiguous, or rejected; only authorized
capabilities can affect the next authority state. Explicit round, capability,
and repeated-state bounds prevent unbounded iteration. Calls add callable
authority; jumps add reachability only.

Gate construction and the WM block harness build now share
`build_backed_dense_generation_catalog_v1`, which derives image digests, exact
64 KiB shard identities, generation geometry, and direct-KSEG physical backing
from the normalized ROM, dense pack, and topology. The build emits the
catalog's canonical-definition digest. The runtime independently reconstructs
the generated dense-only definition and requires that digest before separately
adding captured external executable images. Synthetic tests cover a two-round
unlock, deterministic ordering/deduplication, jump-without-callable authority,
all typed dispositions, identity tampering, explicit bounds, and the resident-
tail identity golden. The eight fixed-point tests and all thirteen WM gate
tests each passed 10/10 consecutive guarded runs. These establish the mechanism
independently of its ROM payoff.

The first ROM-bearing regeneration on 2026-07-31 reached a fixed point in two
rounds. Eighteen exact-transfer findings classified as one authorized target
root, one activation miss, nine ambiguous selections, and seven rejected
requests; the one capability admits `recovered_overlay_2` and the second round
finds its downstream transfers. The caller-attested scorecard moved from the
prior 1,810 `block_aot` / 13 `dynamic_mips` / 0 `unsupported` denominator to
2,047 / 19 / 0. AOT-covered concrete bytes increased 7,240 -> 8,188 (+948),
while the total destination denominator expanded by 243 entries; the six added
dynamic destinations are newly exposed frontier, not regressions from AOT.
The transfer inventory expanded from 2,660 guest / 137 host / 3 open to 2,990 /
137 / 16. This is a real catalog-closure gain, but its nine ambiguous and seven
rejected findings, six open exception vectors, and eight open writer channels
keep `completion_claim = false`.

Read-only NWXE characterization on 2026-07-30 gives the bounded outcome:
`0x800e1bcc` (`jal`) → `0x8013b744` selects exactly
`recovered_overlay_2`; `0x800f1de4` (`j`) → `0x8010211c` has zero compatible
generations and is a typed activation miss; and `0x800e1bb4` (`jal`) →
`0x8013c3c0` remains unauthorized because delay word `0x800e1bb8` is an `sh`
whose effective address is not yet proven outside catalog backing. Thus this
slice closes one callable edge, proves one destination cannot activate from
that source, and leaves one exact address-proof frontier. It does not prove
the loader can reach an overlay, manufacture all-path authority, or use the
observed runtime route as static evidence.

Held-out grading found `misclassified_as_code = 0` on all three: no
exact_aot/block_aot destination lands where the dump says data.

Phase-6 indirect closure was then strengthened on the recovered NWXE overlay
banks (three sound `sltiu`-bounded switch-table recognizers, each with a
near-miss test proving no over-admission): `unresolved_indirect` occurrences
fell 19,196 → 16,366 (−2,830, ~15%) with exact owners held at 6 and wrong
extents at 0, and no candidate-grade regression (OoT/NW4E exhaustive jump
tables rose 230→240 / 223→227). The finding this surfaces: indirect closure is
no longer the binding constraint on the three zero-owner overlays — they are
dominated by `entry_not_authoritative` (987), `owner_missing` (567), and
`partition_ambiguity` (895), which is where owner recovery goes next. Several
remaining indirect sites are irreducibly open (index/base arriving through
function arguments or mutable memory the static analysis cannot bound).

The first integrated run also exposed an existing Phase-6 fixed-point cycle:
NWXE's fourth overlay alternated forever between 96 and 97 exhaustive
indirect sites. Closure now detects a repeated edge-set state, retains only
entries identical throughout the cycle, monotonically revalidates that
intersection, and leaves every oscillating site `Open`. It does not choose a
cycle side by score or iteration order.

The byte-verified `ProgramSnapshotV1` now closes the native resident-bank
passes into one artifact. With only the NWXE header entry as a traversal seed,
the real-ROM gate produces 197 blocks, 27 owner assessments, zero partition
ambiguities/overlaps, and 26 exact + 3 coarse answer-key grading matches with
zero wrong splits. Ten complete compositions serialize byte-identically.

Composition now derives proof-carrying executable evidence from the closure
itself: the union of reached proven-code block intervals becomes typed,
`Proven`-concluded `ExecutableRange` facts (rule
`reached_proven_code_closure`), and owners are re-proven against them. A word
reached by CFG closure from the authoritative entry is demonstrably executed
under the proven mapping; exactly those bytes are claimed — adjacent blocks
merge, but a gap between reached blocks is never bridged, and region scores
play no role (already rejected as a promotion rule). This discharged the
former `not_proven_executable` sole-blocker frontier (27 assessments, sole
blocker for 25). Grading the newly admitted extents end-to-end against the
dump key then exposed two over-claims — one owner truncated before an
unreached trailing `jr $ra; nop` the key attributes to it, one owner smeared
across a non-returning call's fallthrough into the next function's prologue —
so exact-owner proof gained two typed withholding rules:
`interior_candidate_entry` (an unrefuted candidate entry claim strictly
inside the extent) and `trailing_unattributed_code` (unreached non-zero
bytes at the extent end that no entry claim or reached code attributes;
byte-identical neighborhoods were measured with opposite ground-truth
attributions, so no content rule can decide them). The measured NWXE owner
frontier is now:

```text
exact owners                20 of 27 assessments; all 20 extents equal the dump key (hard gate wrong=0)
trailing_unattributed_code   5 assessments (sole blocker for 4)
interior_candidate_entry     2 assessments (sole blocker for 1)
owner_not_contiguous         2 assessments
malformed_block              1 assessment / 31 sites
word_not_proven_code         1 assessment / 9 sites
not_proven_executable        1 assessment (the gap-spanning non-contiguous owner)
```

The OoT boot bank gets the same treatment in `gate_b2`: its snapshot proves
301/306 reached blocks (6,744 bytes, 35 intervals) and admits 32 of 45 owner
assessments (31 at exact linker-map starts within their key slots plus one
proven-`jal`-target interior split, hard gate wrong=0; the linker map derives
each end from the next symbol start, so key extents include trailing padding
and literal data that a code-extent proof must not claim).

The parallel function-independent gate proves all 197 currently reached NWXE
blocks (4,156 bytes) with exact ROM backing. Every discovered block start is
now a canonical leader, so a later-discovered target splits an earlier linear
scan instead of leaving overlapping pseudo-blocks. This is reached-code coverage,
not total resident text coverage: the answer key contains 847 functions and
overlays are still undiscovered. It demonstrates that exact historical
function boundaries are not the lone mechanism for recompilation.

The Rust `BlockPackV1` envelope now emits schema v2 for those proven block
identities, geometry, address spaces, terminators, and per-block digests without
ROM words. Authoritative emission requires the opaque move-only validated
composition; the public snapshot JSON remains diagnostic and its legacy emitter
can produce only a physical-only v1 wire. Materialization
re-verifies the normalized ROM and every block digest, then feeds disjoint
spans directly to the typed sparse arbitrary-PC emitter. The real NWXE gate
emits 197 blocks / 1,039 words, obtains pack SHA-256
`5944f1a0c63523591cbef33c4856c594b2cca38466945bc63da35a7459dace44`
(re-checked by `scripts/gate-determinism.sh`, whose `gate_b2` stdout digest
covers this pack line now that H3 made the gate's inputs env-declared), and
compiles the generated runner with `rustc`. Addresses in gaps receive no
dispatch arm; static and computed transfers into them remain unresolved.
The gate no longer stops at "rustc accepted": it links the emitted runner
into a host binary and executes it against the real pack — duplicate
registration rejected, the entry PC run to its first typed transfer, a
mechanically derived register-only interior PC entered mid-block, the pack
hole/unaligned/unknown-bank entries all faulting typed, a minimum-budget
checkpoint, and a bounded transfer-following dispatch loop. A separate
probe enters `entry+4` (skipping the entry stub's `lui`) and asserts that
the resulting wild store now returns a typed VR4300 `MemoryFault` naming the
faulting PC and its wild guest address `0xffffffffffffb4c0` — the first slice
of U4 (`UNIVERSAL-RUNTIME-PLAN.md`) landed. The probe still fails loudly if
that access stops faulting typed; full address-error/TLB vectoring remains
open U4 scope.

The public `emit_block_program_source` seam converts a reverified pack into
deterministic Rust implementing the boot-harness block contract. Its typed
configuration requires an admitted bank-qualified entry and instruction
budget, every runner is bound to the host-supplied source artifact identity,
and bankless overlapping-VA lookup returns typed ambiguity instead of
guessing; same-bank transfer resolution retains priority. A synthetic
two-bank test compiles and executes the source while proving sparse-hole
faults and runner identity evidence.

The `fn64-discover emit-block-program` command exposes that seam without
making generated game output repository content. It requires the ROM, strict
pack JSON, exact-width uppercase bank/PC values, canonical decimal budget, and
an explicit output path. The ROM-derived source is staged and synced beside
the destination, published without clobbering an existing file, and identified
by a stdout SHA-256/byte receipt. Synthetic CLI integration tests cover
deterministic output, retained overlay ambiguity, wrong ROM/schema/entry,
unknown fields, numeric rejection, output failures, and the unchanged legacy
discovery invocation.

This orders the next work. Proof-carrying resident executable regions are
recovered (above): reached-code closure now feeds typed executable facts and
exact owners are admitted and extent-graded, so the frontier has moved from
"nothing is proven executable" to the boundary-attribution blockers listed in
the histogram. Next, replace the assumption that a function is one contiguous all-code byte
interval with the canonical block/data-object region model already planned in
`DISCOVER-STORAGE.md`. Non-contiguous block ownership is normal when local
jump tables, literal pools, unreachable padding, or split assembly regions lie
inside a historical function extent; it is a Decomp Pack modeling problem,
not evidence that those bytes should be guessed as instructions. Exact
contiguous owners remain the narrow function-AOT admission path, while
`block_aot` provides the mechanically complete execution path.
The runtime `CodeCatalog` now owns sorted, bank-bound, non-overlapping
`CodeSpan` values and resolves them with a binary-search address index. The
real gate re-resolves every packed NWXE word and proves that hole `0x8000043c`
faults as unmapped. `BlockProgram` now atomically pairs a `CodeBank` with the
bank identity embedded in its generated function, rejects mismatches and
duplicates before mutation, and resolves the sparse entry before invocation.
The emitter supplies the registration helper, and the gate's executed
harness (above) enters both the entry PC and a derived interior PC through
that program. Live executor/shell ownership remains
open; the shell does not yet dispatch guest execution through this lane.

The ROM-only multi-scale region gate is now runnable with no manifest, and
its control features use the shared instruction decoder. On NWXE, the held-out
resident text end at ROM `0x4c0c0` is not the top cross-scale transition; a
`0x100` window proposes nearby `0x4b500`, still `0xbc0` bytes early. This
rules out promoting a region-score threshold as the executable proof rule.
The proof path must combine loader/materialization geometry with decoded CFG
closure, typed data/xrefs/relocations, and exact constraint uniqueness. If
more than one code/data partition satisfies those constraints, the interval
stays candidate/open and execution uses `block_aot` or `dynamic_mips` rather
than silently accepting the highest score.

## Metric ladder

No single percentage is called “coverage.” Reports keep these quantities
separate:

1. Normalized physical ROM bytes.
2. Physical bytes assigned to a known file or direct load image.
3. Logical load-image bytes, bank-qualified so overlapping overlays count
   independently.
4. Bytes classified by content kind, including conflicts and unknowns.
5. Executable bytes established by loader/cache evidence or corroborated
   structural analysis.
6. Reachable executable bytes from proven roots.
7. Function-owned bytes: exact, coarse, ambiguous, and unowned.
8. Recompiler-accepted bytes and unresolved direct/indirect transfers.
9. Runtime-executed blocks under named scenarios.

Function-entry precision is exact correct starts divided by emitted candidate
starts. Entry recall is distinct correct starts found divided by known starts.
Neither metric says anything about function ends or total ROM bytes.

### Coverage gate

`gate_coverage` renders the ladder as deterministic text lines from the real
pipeline, one report per supplied ROM. It reads ROM paths from named, declared
env vars — `FN64_DISCOVER_NW4E_ROM`, `FN64_DISCOVER_NWXE_ROM`,
`FN64_DISCOVER_OOT_ROM` — and prints a loud `skip` line for any that is unset,
never a silent omission. There are no default paths into a home directory.

Every quantity comes from the fact database `run_discovery` produces over cited,
answer-key-free table geometry; no per-ROM constant lives in the engine. Each
report prints, on stable-ordered integer-only lines: physical ROM bytes;
physical bytes assigned to a direct load image or a known file; logical
load-image bytes (bank-qualified, overlapping overlays counted independently);
executable bytes and executable banks; entry-conclusion counts across every
proof state (open / candidate / supported / rejected / conflict / proven);
owner-proof coverage (exact vs candidate vs ambiguous, with blocker counts); and
pack blocks/words plus a content digest where a `BlockPack` exists. The
rendering path (`coverage::render_report` / `coverage::pack_coverage`) has unit
tests asserting exact expected strings.

Measured coverage is not proof. A mapped or executable byte count reports what
evidence established for an interval, not that the interval is authoritative for
emission — the owner-proof and block-proof gates remain the arbiters of that.
Running the generic pipeline, `gate_coverage` reports `owner_proof not_run` and
`pack none`; those lines populate only when a later phase has done the
game-specific per-bank interval selection those proofs require. Ten consecutive
runs over all three ROMs produce byte-identical output (SHA-256
`6153e54d4f04af85645795c5e2a5a2192391b4eeb6978dd2d88b44aaedcd07c6`),
re-checkable via `scripts/gate-determinism.sh` when all three ROM vars are set.

## Invariants

- Identity is `(normalized ROM digest, bank, address)`, never address alone.
- A load mapping does not imply executable permission.
- A plausible instruction sequence is a candidate, not proof of code.
- A direct call from proven code proves a callable target; a raw call-shaped
  word in an unresolved region does not.
- Dynamic execution proves existence, never exhaustiveness.
- External evidence is normalized-ROM-digest-bound, schema-versioned, cited,
  and validated by the same code as inferred evidence.
- Game-specific facts live in external manifests or generated fact packs, not
  branches in the Rust engine.
- Generated artifacts contain no ROM bytes.
- Every unresolved direct target, indirect site, overlap, and unknown byte
  interval remains explicit.

## Multi-view analysis

All passes read the same canonical big-endian bytes and emit immutable typed
facts. “Transforms” are independent views; no pass destructively rewrites the
ROM or overwrites a stronger conclusion.

| View | Aggregates/transforms | Output role |
|---|---|---|
| Header/boot | byte-order normalization, header fields, exact admitted IPL3 identity, complete boot copy | proven initial mapping and entry; otherwise typed Open frontier |
| Loader/DMA | PI register writes, libultra DMA-call argument slices, source/destination/length triples | candidate or proven load images |
| Record structure | repeated strides, aligned range triples, sentinel/count use, loader field provenance | table shape and record semantics |
| Code shape | ISA validity, delay-slot legality, branch-target coherence, return/call density | candidate executable intervals |
| Pointer shape | RDRAM/VROM/ROM range density, alignment, HI/LO references, bounded arrays | pointer/table candidates |
| Byte statistics | zero runs, byte diversity, entropy, repeated blocks, adjacent-window derivatives | padding/assets/change-point candidates |
| Graph | recursive reachability, calls, tails, dominance, value sets | proven code and transfer frontier |
| Cross-ROM | relocation-masked words, opcode skeletons, unique full-body matches | transferred candidates and identities |
| Dynamic | PI DMA, active banks, executed PCs, indirect targets, table writes | observed facts and activation evidence |
| Tool adapters | Splat/spimdisasm partitions and symbols | independent candidates only |

Run code, pointer, and byte-statistic views at multiple window sizes. A 64-byte
window can see a short table or stub; 256-byte and 4 KiB windows stabilize
density; adjacent-window derivatives propose boundaries. Raw scores stay in
the artifact. Promotion uses named rules over independent evidence, not a
trained opaque threshold.

## Work stages

### 1. External evidence and coverage — in progress

- Serializable TOML manifest bound to normalized SHA-256.
- Data-driven bank naming; no function pointers in manifest-compatible table
  inputs.
- Separate `RomMapping` and `ExecutableRange` facts.
- Reject unaligned, overlapping, unbacked, or uncited executable claims.
- Emit physical, logical, executable, and entry-state coverage separately.
- Next: remove remaining grading-only per-ROM constants and validate manifest
  determinism over ten runs.

### 2. Region classifier

- Emit multi-scale feature windows and adjacent-window deltas.
- Detect code/data/pointer/zero/opaque candidates without promoting them.
- Grade proposed executable boundaries separately from function starts.
- Calibrate on multiple ROMs and retain at least one holdout ROM.
- Feed only corroborated executable intervals to candidate harvesting.

The first generic prototype now emits deterministic 64-byte, 256-byte, and
4-KiB views for control transfers, target coherence, returns, pointers, zero
words, diversity, and adjacent-window derivatives. Directional code-to-data
scores ranked three held-out overlay text ends near the top at one or more
scales, but the resident-bank boundary remained poor because physically
adjacent bytes belong to another overlay's code. Region scores therefore stay
candidate-only: loader and bank activation semantics are mandatory evidence,
not a refinement that content statistics can replace.

### 3. Mechanical load-image recovery

The public libultra `osPiStartDma` contract exposes device address, RDRAM
address, byte count, and direction. The Programming Manual's overlay example
also invalidates text/data caches, DMAs a ROM interval to a segment start, and
clears BSS. These give generic observable shapes for static slicing and dynamic
tracing, not game-specific table layouts:

- Recognize direct PI register programming and known ABI calls.
- Backward-slice ROM source, RDRAM destination, and length.
- Find loops or call sites that load constant-stride records.
- Recover table base, record count/sentinel, stride, and field offsets from
  actual uses.
- Confirm mappings with structural bounds and, when available, PI DMA traces.
- Infer text end from instruction-cache invalidation range; infer data and BSS
  from data-cache invalidation and clear ranges.

References: the public libultra `osPiStartDma` Syntax/Description
([manual entry](https://ultra64.ca/files/documentation/online-manuals/man-v5-1/n64man/os/osPiStartDma.htm)),
the public libultra `osEPiStartDma` Syntax/Description
([manual entry](https://ultra64.ca/files/documentation/online-manuals/man-v5-2/allman52/n64man/os/osEPiStartDma.htm)),
[N64 Programming Manual, overlays](https://ultra64.ca/files/documentation/online-manuals/man/pro-man/pro10/10-03.html),
and [PI register definitions](https://ultra64.ca/files/documentation/online-manuals/man/header/rcp.htm).

The first strict entry-stub recognizer now proves both end-pointer and
countdown zero-fill loops, including complete per-stride store coverage and
the post-clear constructed jump without naming that jump's source-level role.
On the two current AKI grading ROMs it derives, rather than embeds, the
different BSS ranges and reaches the same held-out entry target. Ten
consecutive real-ROM gate runs produced the same output SHA-256
`5a67f5e471bad44bbb85aba27decd4ac831d93f2a24f0de1b329c3393bfec921`,
re-checkable via `scripts/gate-determinism.sh`.
This is a narrow loader fact, not general overlay discovery. Correct PI/EPI
slicing and normalized record-use recovery now exist, but the interprocedural
producer connecting real wrapper loads to those inputs remains open.

Static PI-DMA slicing now keeps the two public libultra APIs distinct. Direct
`osPiStartDma(OSIoMesg *, priority, direction, devAddr, vAddr, nbytes, mq)`
uses o32 `$a2` for direction, `$a3` for device address, and caller-stack
offsets `+0x10`/`+0x14` for RDRAM pointer/byte count. Direct
`osEPiStartDma(OSPiHandle *, OSIoMesg *, direction)` instead recovers
direction from `$a2` and geometry from message fields `+0x08`, `+0x0c`, and
`+0x10`; stack-local and statically addressed messages are supported. Both
slicers evaluate the call delay slot, stop at prior control-flow boundaries,
and cap their backward window at 64 words. Each constant operand has typed
provenance; missing fields, loads, unsupported writes, aliasing stores,
invalid directions, zero lengths, and address/range failures remain explicit
blockers. KSEG0/KSEG1 pointers are checked into the configured physical RDRAM
domain. Even a complete slice is a `StaticPiDmaCandidate`: static bytes do not
prove reachability, asynchronous completion, or EPI handle-to-ROM mapping.
The open integration steps are symbol/signature authority for the callees,
interprocedural affine record-use recovery, handle-state recovery, and dynamic
completion corroboration.

The first symbol-search prerequisite is now isolated in `host_bindings` as a
candidate filter and remains unwired. Authoritative bank-qualified roots,
`ProvenCode`, and executable ranges constrain where it may look, but they do
not authenticate either libultra symbol. A unique `osEPiStartDma` raw-prefix
shape candidate must exist before an `osPiStartDma` shape candidate can be
returned; proving the EPI prefix's paths and owner remains open. The PI entry
block must end in the CFG's exact direct call to that EPI shape, and relational
dataflow must populate priority, status, return queue, RDRAM address, device
address, and byte count from the public seven-argument o32 ABI. Unmodeled GPR
writes clobber tags, unmodeled or possibly aliasing stores reject the shape,
and explicit root/call/block/work limits fail open with counts and samples.
The result type also retains unresolved cart-handle/device-base authority:
`devAddr` is not a physical-ROM coordinate until that prerequisite is proved.
Missing, ambiguous, unreachable, capped, or byte/CFG-inconsistent evidence
creates no mapping or installed host binding.

The next structural stage is implemented independently of instruction
matching. `load_table_use` accepts immutable, bank-qualified word loads whose
semantic roles were established by the public overlay sequence; it normalizes
biased record pointers, deduplicates observations, requires stable role
offsets and stride, and validates ROM/text/data/BSS range relations. A crucial
completeness gate is explicit: consecutive records are only candidates unless
preceding loop analysis independently proves the exact table base, count, and
stride. Thus a four-record subset of a five-record table cannot be mislabeled
complete. This pass proves table geometry and role layout only, never
executable permission. Real wrapper-summary production and unique source-bank
mapping translation are still open.

### 4. Trace ingestion

- The stable JSONL schema is normalized-ROM-digest-bound, strictly sequenced,
  and has explicit header/completion records.
- PI DMA, executed PC, indirect transfer, active-bank generation, and watched
  table-write events now ingest into typed observations.
- Unknown bank identity is preserved instead of guessed. Producer
  exhaustiveness claims are separate, bounded to sequence intervals, and do
  not convert observations into global completeness.
- `fn64-discover --trace` accepts multiple inputs, rejects duplicate trace
  identities, and embeds deterministic reports in the discovery artifact.
- Treat a PC observation as code existence in the active bank.
- Treat observed indirect targets as non-exhaustive.
- Generate targeted scenarios from the unresolved frontier.

Producer v1 exists (2026-07-18): `tools/mupen-trace/mupen_trace.c` drives the
DEBUGGER=1 mupen64plus-core 2.6.0 build in documented single-step mode through
the public `m64p_debug` API only, and emits schema-v1 JSONL — digest-bound
header, bounded executed-PC window from the entrypoint, watched-value
transition records for the NW4E selector flag/mode cells (observed values
only, no fabricated write-PC attribution), and a completion record whose
exhaustiveness claim covers executed PCs alone. A 500,000-step NW4E boot
capture is byte-identical across three runs (SHA-256 `c19fd46c…`). The
watched cells hold boot-copy residue until the entry stub's zero-fill sweeps
them (mode at sequence 167,912, flag at 191,088) — independently confirming
the earlier "transient" explanation; the dispatcher's own stores lie beyond
this bounded window. `gate_trace` (env-declared ROM + trace paths, 10/10
byte-identical) ingests 500,004 facts through the existing path and
classifies 1,868 unique observed resident PCs against the static baseline:
639 inside proven-closure code, 1,035 corroborating exploratory candidate
words, 194 previously-unclassified words of new code-existence evidence, and
zero static-versus-execution conflicts; three unknown-bank PCs are the
general exception vector. The `FactDb` adapter that this originally lacked
now exists (`trace::fold_executed_pcs_into_fact_db`): the same 500k capture
folds into 499,997 facts / 1,868 Supported code-existence conclusions.

A breakpoint-accelerated driver (`tools/mupen-bfs/mupen_bfs.c`) was built to
reach past the ~500k single-step wall to the selector dispatcher's flag
stores. Measured negative finding, recorded so it is not re-attempted the
same way: on this DEBUGGER=1 macOS-arm64 core build, `M64P_DBG_RUNSTATE_RUNNING`
does NOT free-run once a breakpoint is installed — `sample` backtraces show it
parks on a per-instruction semaphore and only `DebugStep()` advances it, so
every instruction still costs one step round trip. Execution breakpoints at
the byte-verified dispatcher PCs plus a write watchpoint on `0x800a10b0` do
fire correctly and deterministically (4/4 byte-identical runs): the driver
reaches the init store and the R2 flag load with flag/mode at their zero-init
values, but a 114M-step / ~10-minute run never reached the R3/R5/loop branches
or any overlay flag store — the interpreter spends that time in a
hardware-timing poll loop. Deep selector-state observation is therefore
blocked by interpreter speed, not driver correctness. **This is not an arm64
dynarec gap** (a native Apple Silicon dynarec was added to the pinned core
build later, see `tools/mupen-trace/README.md`) — the debugger path itself
forces the pure interpreter, because `EnableDebugger` requires
`R4300Emulator=0` regardless of host architecture. The actual unblock is
either a full-speed capture that skips the debugger entirely (see
`FN64_CAPTURE_SECONDS` below) or a coarser breakpoint/watchpoint mechanism
that needs fewer step round trips, not a faster host CPU. Traces contain
executed-PC sequences from a user's ROM and stay out of git; compiled drivers
are gitignored build artifacts.

### 5. Function entries and ownership

- Promote direct calls only when their source instruction is proven code.
- Resolve finite `jalr` sets and bounded jump tables through value-set analysis.
- Add table/callback roots only after table semantics are proven.
- Use prologues, external tools, and cross-ROM homology as candidates.
- Partition reachable blocks into non-overlapping owners, retaining shared
  tails and interior callable entries explicitly.
- Report exact/coarse/ambiguous/unowned byte counts per bank.

The conservative proof boundary is implemented in
[`DISCOVER-OWNER-PROOF.md`](DISCOVER-OWNER-PROOF.md). Its result type carries
an exact extent only after entry authority, CFG and delay-slot validity,
unique ROM backing, proven executable coverage, incoming-edge exclusion, and
indirect closure all hold. Every failed premise remains a typed candidate or
ambiguity blocker. It is not wired into the real-ROM gate yet, so the measured
exact-owner count above remains zero. The current global indirect-closure rule
is intentionally strict until facts can represent a bounded target domain
that excludes an otherwise unrelated owner.

### 6. Boundary and recompiler validation

N64Recomp currently consumes a list of sections and per-function address/size
metadata, and fn64's Rust recompiler likewise slices exact words for each
function. Exact extents are therefore required by today's emitter.

Assembly/relink round trips validate byte decoding, relocation reconstruction,
and that a proposed partition covers the expected bytes. They cannot prove a
function boundary by themselves: many different aligned partitions reassemble
to the same bytes. Boundary proof must come from control-flow ownership and
callable-entry evidence.

Build a bank/basic-block recompiler mode in which correct execution does not
depend on reconstructing original source-level function partitions. Indirect
dispatch is keyed by `(BankId, PC)` and may target any admitted block. This
makes functional recompilation possible before every historical boundary is
known, while the decompilation pack keeps stricter exact-owner requirements.
[N64Recomp's documented input model](https://github.com/N64Recomp/N64Recomp)
confirms that its current path is function-metadata driven, so this is an fn64
closure mechanism rather than metadata we can delegate to that input model.

Add an explicit MIPS interpreter or equivalent semantics-preserving dynamic
backend for code whose bytes are generated, decrypted, decompressed, relocated,
or selected only at runtime. This is not an external emulator and not a silent
stub: it shares fn64's typed RDRAM/register/runtime state, traps unsupported
hardware behavior, and emits promotion traces so repeated fallback blocks can
become new AOT candidates. The same closure rule applies to custom RSP code if
the runtime cannot otherwise execute the task faithfully.

### 7. External tools and cross-ROM transfer

- Keep the core fact database, loader analysis, CFG, and grading in Rust.
- Use [Splat](https://github.com/ethteck/splat) and
  [spimdisasm](https://github.com/Decompollaborate/spimdisasm) as candidate
  providers and assembly/decomp consumers.
- Use Ghidra decompiler output only as candidate type/control-flow evidence;
  it is not a boundary oracle.
- Port the existing relocation-masked AKI fingerprint and opcode-skeleton
  matching into Rust.
- Require uniqueness, full-body validation, bank compatibility, and a clear
  runner-up margin. A transferred name never proves an extent on its own.

The external-tool path now has two validated producers: spimdisasm
function-info normalization and a headless Ghidra raw-bank candidate run. Both
emit candidate-only, bank-qualified, digest- and lineage-bound claims. Ghidra
passed ten deterministic synthetic runs with same-VA banks isolated and
seeded/unseeded results distinct. A seedless run over the retained Banjo boot
bank produced 123 claims from 61 entries; candidate-seeded exploratory CFG
coverage added 952 words (+28.7%) over the native baseline without exhausting
the analyzer or proving any owner. Classification against the locally produced
baseline snapshot finds 53 already reached entries (39 targeted by ProvenCode
direct calls, 14 reached without a call relation), eight unreached entries,
and zero exhaustive resolved-call targets; snapshot states are 35 Candidate,
22 Supported, and four absent. `candidate_corroboration` now admits that
sidecar only through the exact completed queue/attempt/runner receipt chain,
including the retained request, provider output, configuration, evidence,
tool manifest, and snapshot bytes. The resulting capability is move-only and
reports analyzer completeness as `Unknown`; it has no FactDb, partition,
owner, or traversal-root conversion. The next expansion is independent native
corroboration of the unmatched candidates, not more function-start voting.
The capability proves internal receipt consistency; interpreting snapshot
relations as native evidence additionally requires authenticating the snapshot
producer, which this receipt bundle does not currently do.

The bounded follow-up now seeds only those eight baseline-unreached entries.
That reduced pass visits 3,574 words: 2,622 overlap the baseline and all 952
words newly found by the earlier 61-entry union remain present. It also sees
628 blocks, 120 conditional direct calls, one tail transfer, and four indirect
sites. Four roots already have one native Candidate conclusion and four are
absent from the native entry facts. These are candidate-seeded diagnostics,
not independent corroboration; the result proves that the other 53 tool roots
can be removed from this feedback loop without losing marginal coverage on
this bank.

The spimdisasm path also has a strict cached per-bank reference interchange.
Its adapter-owned JSON/JSONL contract covers block starts, direct references,
HI/LO pairs, and typed data candidates with exact `BankId` and VA/VROM
geometry. Tool version/build/source, configuration, provider output, and bank
input are digest-bound; the cache key additionally binds the normalization
algorithm. Records are bounded, canonical, sorted, and unique, while repeats,
inconsistencies, and overlapping data candidates fail closed. The receipt
stores identities, geometry, counts, and digests only, never paths or provider
content. The normalized output is candidate-only and has no native-fact
ingestion path; wiring these candidates into the canonical graph and measuring
their marginal impact remain open.

### 8. Pack emission and end-to-end gate

- Emit two views from one fact snapshot: a Recompiler Pack containing only
  admitted banks/owners/transfers, and a Decomp Pack containing matching
  assembly plus provenance-bearing symbols, xrefs, relocations, data objects,
  prototypes/types, stack frames, and Splat/Ghidra inputs.
- Let RE tools and analyst manifests enrich the Decomp Pack without silently
  strengthening Recompiler Pack proof state.
- Emit an execution-closure table covering every admitted bank-qualified
  destination as `exact_aot`, `block_aot`, `dynamic_mips`, or `unsupported`.
- Recompile with both fn64 emitters where supported.
- Treat compiler diagnostics as new frontier facts, not automatic patches.
- Run lane parity below its documented valid horizon, then scripted live
  framebuffer/audio captures.
- Require deterministic output across ten clean runs; concurrency-sensitive
  stages require twenty.
- Require zero `unsupported` execution destinations for a full-game build;
  report dynamic fallback entries and runtime counts separately rather than
  hiding them in a percentage.

## Feedback loops

| Loop | Target time | Gate |
|---|---:|---|
| Synthetic feature/proof tests | under 1 s | exact fact/state assertions |
| `cargo test -p fn64-discover` | under 10 s | all unit and determinism tests |
| Corpus entry/boundary grade | under 15 s | per-provider and per-bank deltas |
| One-bank pack/recompile smoke | under 1 min | no missing direct targets or overlaps |
| Scripted emulator scenario | minutes | named frame/audio/trace comparison |
| Execution closure | incremental | zero unsupported `(BankId, PC)` destinations |

Every experiment records inputs, normalized digest, algorithm version,
metrics before/after, and whether it changed authoritative facts or only
candidates.

## Immediate next experiments

1. Feed strict typed entry-stub observations into reachable CFG facts while
   retaining the post-clear transfer's source-level role as a candidate.
2. Compose the static PI/EPI slices through wrappers and recover affine
   load-table record use; require a unique overlay-field interpretation.
3. Connect one headless black-box emulator to the existing trace/probe schema
   and verify bank-qualified PC, indirect-target, and PI DMA observations.
4. Run CFG-structure homology on the byte-homology unmatched remainder.
5. Re-run NWXE, NW4E, and OoT grades, then hold out another AKI ROM for the
   first no-descriptor evaluation.
## Prioritized unblocking roadmap (any-ROM decomp/recomp/ports)

Ranked by expected slope toward running, then decompiling, an arbitrary N64
ROM. Items already scheduled elsewhere cite their home; new items state what
must exist before they can start. Ranking is a bet, not a proof — each item
must still clear the same measurement bar as every experiment above, and a
rejected result gets recorded with its numbers exactly like the
aligned-pointer-run rejection.

1. **Instrumented trace producer** (extends stage 4; U7's evidence engine).
   Every hard static frontier — open indirect calls (569 on OoT), overlay
   activation, runtime-built tables, selector state — becomes an observation
   under execution. The ingestion schema and typed observations already
   exist (`trace.rs`); the missing half is a repeatable headless producer
   emitting bank-qualified PCs, PI DMA, and indirect targets, then
   savestate-forking, coverage-guided exploration with explicit
   natural-versus-forced reachability labels. The debugger-driven
   Mupen64Plus probe is the manual precursor; the product is a scripted one.
2. **`dynamic_mips` fallback** (defined in this plan's closure taxonomy;
   groundwork implemented, universal coverage incomplete). The existing
   `dev-interpreter` covers the first integer/control/memory slice, the typed
   fallback dispatcher maps `BlockExit` into it, the live executor can resume
   one game thread through that lane, and interpreted MMIO already reaches the
   modeled device fabric. What remains is production-wide admission and
   closure for every bank-qualified destination plus the instruction and
   exception classes still typed unsupported (including FPU/COP0/exceptions).
   Once those gaps close, static admission failure can run instrumented instead
   of faulting: AOT coverage becomes an optimization, and fallback executions
   emit promotion traces that feed item 1's evidence loop back into AOT
   admission.
3. **Corpus-scale homology** (extends stage 7). Pairwise relocation-masked
   matching already measures 98.75–99.64% precision. Generalize to an
   N-ROM mutual-labeling fact corpus: every N64 ROM links one of a small
   set of libultra/SDK builds, engine families share most of their code,
   and each onboarded ROM both consumes and contributes identities. This is
   also the clean-room-safe substitute for signature databases.
4. **Compiler-idiom-exact recognizers** (new; needs a per-ROM compiler
   classifier first). Nearly all N64 code came from IDO 5.3/7.1 or a known
   GCC; per-compiler prologue/jump-table/scheduling idioms are
   near-deterministic, unlike the generic patterns that scored 25.9%
   prologue precision on NWXE. Detect the compiler, then apply its exact
   idioms as candidate providers — measured against the answer keys before
   adoption, like every provider.
5. **Relocation recovery by differential comparison** (new; feeds the
   Decomp Pack). The same overlay observed at two load addresses — or the
   same engine code across ROM revisions — mechanically reveals pointer
   words: values differing by exactly the load delta are relocations.
   The AKI family's shared-engine corpus is the natural first target.
6. **Decompressor provenance via dynamic execution** (unblocks compressed
   ROMs generally; depends on items 1–2). Run the ROM's own decompressor
   in the instrumented lane and bind output bytes to source bytes — the
   proof-carrying materialization transform the snapshot design requires
   before virtual/compressed backing can enter `ProgramSnapshotV1`.
7. **Cache-op text bounds and thread-entry harvesting** (extends stage 3 /
   Phase 3). `osInvalICache`-range slicing proves text extents; the NW4E
   thread-registration shape (entry address materialized into `$a2` for a
   create/start pair) generalizes into a callback-entry harvester once
   item 3 identifies the thread-create callee per ROM.

**Brute-force enumeration lane** (cross-cutting; MIPS-III's fixed-width
aligned encoding makes exhaustive hypothesis enumeration cheap, and the
rule is always enumerate-then-constrain, never promote-by-score):

- **Delta-voting mapping inference** (`delta_vote.rs` / `gate_delta_vote`,
  landed 2026-07-18): for a candidate region, enumerate VA-delta hypotheses —
  narrowed by the region's `lui` upper-half histogram, with a full aligned
  sweep as fallback — and vote over mapping-independent constraints: absolute
  `jal`/`j` targets landing on `addiu $sp,-N` prologues or known entries
  (votes counted over *distinct* targets, so a popular callee cannot
  manufacture domination), `%hi/%lo` pairs landing in mapped space
  (corroboration only, plateau-shaped), branch targets staying in-region
  (delta-invariant, used as a filter not a discriminator). This is the
  mechanized form of the NW4E selector VA disambiguation. Admission requires
  the unique top with ≥3 prologue votes AND ≥2× the runner-up; a near-tie
  stays open. **Graded held-out on NW4E's five overlays (`va_start` used only
  to grade, never fed to inference): 5/5 admitted-correct, 0 open, 0 wrong,
  margins 3.1×–9.7×; full-sweep mode admits the identical deltas, so the
  narrowing loses nothing.** NWXE is not graded — its overlay ROM intervals
  need a byte-verified descriptor table or a descriptor-free recovery that
  does not yet exist, so the gate states that frontier rather than guessing
  regions (that recovery is the remaining step toward NWXE's "overlays are
  absent" limitation).
- **GP-base voting** (`gp_base.rs` / `gate_gp_base`, landed 2026-07-18):
  recover the IDO small-data `$gp` base by voting over boot `lui/addiu $gp`
  constructions or an access-offset histogram, admitting the unique
  dominating base only. **Both AKI titles grade OPEN**, and that is the
  disciplined result: NW4E and NWXE resident code contain zero real `$gp`
  constructions and only 6–7 `off($gp)` decodes each, which are data
  misread as code (an unaligned NWXE histogram winner was rejected by a
  word-alignment gate rather than promoted by score). The mechanism's
  positive path is proven by synthetic tests; on these ROMs there is simply
  no gp-relative small-data base to recover, reported as OPEN with numbers
  rather than fabricated.
- **Forced micro-execution sweep** (with items 1–2): execute every
  candidate block under synthesized states in the instrumented lane to
  observe computed-jump targets; results carry the forced-synthetic label
  and never claim natural reachability.
- **All-window rolling-hash corpus matching** (with item 3): reloc-masked
  hashes of every aligned 64-word window across the corpus find shared
  code without needing function boundaries first.

The cautionary precedent stands: the aligned-pointer-run rejection (3.10%
precision) is what enumeration WITHOUT constraint validation produces; the
lane exists because enumeration output feeds validation, not because
enumeration is evidence.

Standing background track, unchanged by this ranking: U2–U6 device/RSP/RDP
closure in `UNIVERSAL-RUNTIME-PLAN.md` — ports need runtime fidelity
regardless of how discovery evidence arrives.

Explicitly not on this list: content-statistics promotion (rejected twice by
measurement: the aligned-pointer-run collapse and the region-score boundary
miss) and LLM-derived facts (the pipeline's zero-LLM property is what makes
its proofs auditable).

## Experiment impact ledger

One row per experiment, one column per ROM, cells holding the measured
deltas that experiment produced on that ROM (combined candidate
precision/recall unless stated). "n/m" = not measured there — absence of a
measurement is recorded, never implied. Dispositions: **adopted** (feeds the
canonical pipeline), **candidate-only** (produces candidate/exploratory
evidence, never authoritative facts), **external-evidence** (measured with
caller-supplied inputs the engine does not infer yet), **rejected** (kept
only as its kill numbers). Sources: the experiment paragraphs above; this
table consolidates, it does not re-measure.

| Experiment | OoT | NW4E | NWXE | Disposition |
|---|---|---|---|---|
| Canonical Zelda overlay-relocation facts | 35,646 facts retained; causal delta not retained | 0 facts retained | 0 facts retained | adopted as inert provenance; MM retains 46,993 facts; pre-ingestion receipts are missing, so current coverage delta is **not verified** and no pointer-to-root promotion follows |
| Stage-1 syntactic effect inventory | 100 obvious external-effect sites; 1,427 memory addresses open | 136; 2,617 open | 136; 2,585 open | adopted as standalone negative classifier; 9-ROM panel measured, absence is not a purity/closure certificate |
| D1.5 load-image/file tables | combined 62.29%/0.82% → 90.57%/72.32% | n/m (uses descriptor path) | n/m | adopted |
| D2 value-set closure + identity audit | precision 90.57% → 98.69%, recall flat 72.32% (JalTarget 82.12% → 97.76%) | 44.69% → 48.44% prec, 89.04% → 89.71% recall | no change (36.36%/28.50%) | adopted |
| Descriptor-table mapping | n/m | 48.44%/89.74% (baseline of its rows) | 49.95%/86.86% vs 36.40%/28.54% boot-only | adopted (shape is data input) |
| Held-out text-interval filter | n/m | +33.97pts prec / −1.60pts recall (82.41%/88.11%) | +31.36pts prec / −2.74pts recall (81.31%/84.11%) | external-evidence (inference open) |
| Aligned-pointer-run harvest | n/m | 3.10% precision | 2.34% precision | rejected |
| Multi-scale region scores | n/m | n/m | resident text end missed by 0xbc0 at best scale | rejected as promotion rule; candidate view retained |
| Cross-ROM byte homology | n/m | ←99.64% prec / ≥15.99% recall LB | ←98.75% prec / ≥22.65% recall LB | adopted (candidate provider) |
| spimdisasm adapter | n/m | n/m | entries 91.99%/97.64%; extents 80.53% exact | candidate-only |
| Entry-stub recognizer | n/m (OoT boot closed via HI/LO jr) | BSS + main entry derived | BSS + main entry derived | adopted |
| Selector VA correction + xref sweep | n/m | dispatcher identity fixed (+0xC00 error), 8-store inventory graded | n/m | adopted (evidence, no P/R metric) |
| Reached-closure executable regions | 32/45 owners admitted (boot bank) | n/m | exact owners 0 → 20/27, wrong=0 held | adopted |
| Pack execution harness | n/m | n/m | round trip executed; typed faults/budget/hole validated (depth, not P/R) | adopted (validation) |
| Ghidra candidate discovery | Banjo boot bank: 61 entries / 123 claims; exploratory CFG +952 words (+28.7%); analyzer completeness unknown | n/m | n/m | candidate-only, receipt-bound; native corroboration open |
| Ghidra computed-flow candidates | OoT boot: all 3 native sites observed; exhaustive target 1/1 exact; 2 open sites remain targetless; 7 Ghidra-only sites, including a 6-target switch | n/m | answer key places extras in `__osException` / `__osDevMgrMain`; no production authority | adopted as schema-v3 differential input; containing-entry authority and native replay remain open |
| Trace producer v1 (500k-step boot window) | n/m | 1,868 executed resident PCs; 639 in proven code, 1,035 candidates corroborated, 194 previously-unclassified; 0 conflicts | n/m | adopted (observed evidence) |
| Trace→FactDb adapter | n/m | ingestion delta 0 → 499,997 facts / 1,868 Supported code-existence conclusions / 478 corroborations / 0 static-data conflicts | n/m | adopted (Supported, distinct evidence class) |
| Delta-voting VA-mapping inference | n/m | 5/5 overlays admitted-correct, 0 open, 0 wrong (margins 3.1x-9.7x) | not graded (no NWXE overlay regions yet) | adopted |
| GP-base voting | n/m | OPEN (0 real $gp constructions; 25 off($gp) = data-as-code noise) | OPEN (7 accesses; unaligned histogram winner rejected) | mechanism adopted; no base to recover on these ROMs |
| Overlay region discovery (descriptor-family search) | n/m | 5/5 regions recovered from ROM alone (table @0x53988 found without being handed it), delta_vote 5/5 correct | **4 overlay regions recovered @table 0x48a68, 100%/100%, delta_vote 4/4 correct, 0 wrong; integrated D1 36.396867%/28.542179% → 49.976448%/86.895987%; a 2nd candidate table correctly rejected** | adopted — mechanically opens NWXE overlays |
| Exact-owner proof on recovered NWXE overlays | n/m | n/m | 6 exact owners (from 0), 0 wrong extents; 22,562 reached blocks, 475,740 proven-executable bytes; dominant blocker unresolved-indirect (614 sole) | adopted — first proof-qualified overlay ownership |
| VROM overlay recovery (file-table resolution) | **OoT: file table @0x7430 recovered (=dmadata); 414 overlay regions, 100% precision / 88.5% recall (actor+kaleido tables admitted)**; SM64 correctly 0 (negative control); GE/PD 0 ungraded | n/m (AKI physical path unchanged) | n/m (unchanged) | adopted — overlay recovery now crosses engine families (AKI + OoT); effect/gamestate tables below 2-region floor stay open |
| OoT end-to-end with recovered overlays (gate_d1_oot_overlays) | **B mechanical NOW EQUALS C hand-geometry EXACTLY: 99.567%/72.331%** (was 48.450%→69.449%→72.331% over 3 steps); all 468 overlay regions recovered (actor 426/426, effect 36/36, gamestate 4/4, kaleido 2/2), 0 wrong | n/m | n/m | thesis proven: mechanical recovery matches hand-encoded overlay geometry exactly, no precision loss, held-out |
| Execution-closure scoreboard (gate_closure), retired pre-whole-ROM baseline | OoT (boot): block_aot 287, dyn_mips 73, **unsupported 6** | NW4E: block_aot 22051, dyn_mips 892, **unsupported 11** | NWXE: exact 95, block_aot 17622, dyn_mips 2169, **unsupported 20** | superseded — retained only as history; the 6–20 headline was withdrawn after whole-ROM OoT composition and is not current evidence |
| Multi-bank cross-overlay owner authority | n/m | n/m | exact_owners 6→7, wrong 0; entry_not_authoritative 987→273 (−714) | adopted — real but exposes partition owner-span construction (owner_missing +578) as next lever |
| Backward-slice indirect resolution (angr pattern, BSD-2) | 1 NW4E site Open→Bounded; precision unchanged | (see NW4E) | wrong 0, all 399 open sites stay open — PROVEN irreducibly static (vtable/return-value jalr = AKI dynamic dispatch) | adopted (sound, robustness) — instrumented negative: 16,366 unresolved_indirect are dynamic_mips territory, not static |
| Corpus call-graph propagation (BinDiff MD-index, Apache-2) | n/m | ←591 body-hash seeds + 44 propagated, 100% precision, 0 wrong | (matched vs NW4E) | adopted — self-corrected from 13.45% (positional-only) to 100% by requiring body-hash corroboration; the propagation engine for corpus homology |
| Relocation-accuracy grade (Ramblr concept) | 280 recovered refs, 50.4% misclassified vs function-symbol key proxy | n/m | n/m | adopted (baseline metric) — Decomp-Pack readiness; proxy grade (key has symbols not full relocs), a number to improve |
| Partition owner-span construction (authoritative splits) | n/m | n/m | NWXE overlay exact_owners 7→46, wrong 0; owner_missing 1145→229 (−916, −468 sole); gate_b2 resident frozen | adopted — splits an owner at a proven interior callable entry (j-vs-jal preserved); next blocker is unresolved_indirect (dynamic, per #21/#28) |
| Corpus-scale N-ROM homology | ←OoT names propagate onto unlabeled AKI functions | ←(shared libultra) | ←(shared libultra) | adopted — 6-ROM corpus, 635 identities, 100% held-out precision, 0 cascade; 5 libultra kernel routines span AKI+Zelda engines; the superlinear payoff (a labeled ROM labels others) |
| Assembly-text serializer + Phase-8 round trip (asm_emit.rs) | 32/32 OoT boot-bank owners emit .s that reassembles byte-identical to ROM (held-out, dump never opened) | n/m | n/m | adopted — the Decomp-Pack assembly proof; re-decodes every word through the shared decoder (no 2nd authority), symbols only from proven catalog, unresolved stays .word; UNBLOCKS #25 matching-decomp |
| Content-consumer data/code discriminator | 2/10 open words correct (20%) | n/m | (see OoT) | DROPPED as shippable — root-caused: 8 wrong Pointers = __osExceptionPreamble idiom, Code signal redundant with cfg.rs; documented dead-end, candidate-only module retained unwired |
| NWXE overlay owner recovery via entry-authority | n/m | n/m | 6→6 (measured negative): entry_not_authoritative/owner_missing have sole_blocker=0, 818/987 roots authorized only by cross-bank jals a single-bank composition can't prove | valid negative — real lever is multi-bank composition (deferred snapshot feature), not entry-authority; 2 guard tests lock the sound exhaustive-jalr boundary |
| dynamic_mips → real device (interp MMIO seam) | n/m | n/m | n/m | adopted (groundwork): interpreted lw/sw of PI_STATUS reads busy→idle across a real DeviceFabric DMA deadline and acks a PI interrupt, through the SAME modeled device authority (port trait, no second authority); hole-stays-fault with MMIO window present; rung suite unchanged; AOT lane untouched |
| Phase-6 indirect closure (switch-table precision) | jump tables 230→240 exhaustive, precision/recall unchanged | 223→227 exhaustive, unchanged | unresolved_indirect 19196→16366 occurrences (−2830), exact_owners 6→6, wrong 0 | adopted — sound (3 near-miss soundness tests); remaining sites blocked by entry_not_authoritative/owner_missing/partition_ambiguity, not indirect |
| dynamic_mips → live executor seam | n/m | n/m | n/m | adopted (groundwork): ExecutorAction maps BlockExit→scheduling decision from exit variant only (AOT/interp indistinguishability is type-level); executor drives fallback in one GameThread resume; hole-stays-fault + single-runnable proven; rung suite unchanged |
| dynamic_mips fallback dispatcher | n/m | n/m | n/m | adopted (groundwork): interpreter wired behind BlockExit, byte-equivalent to AOT lane; hole-stays-a-fault safety proven; typed EvidenceClass; FPU/COP0/exceptions typed-unsupported |
| dynamic_mips interpreter (first slice) | n/m | n/m | n/m | adopted (groundwork): integer/control/memory ops, byte-equivalent to the AOT lane by differential test; FPU/COP0/exceptions typed-unsupported (open) |
| Answer-key corpus intake | n/m | n/m | n/m | infrastructure only: Banjo 60-row override key parsed (55 fn), PD key absent upstream — no grading yet (ROMs not present) |

Maintenance rule: every future experiment lands a row here in the same
commit as its adoption or rejection, with its per-ROM cells filled or
explicitly n/m. An experiment measured on one ROM is not presumed to
transfer; the empty cells are the transfer-measurement backlog.

## Research intake (2026-07-18)

License-verified external resources, fetched from each project's canonical
LICENSE file (not asserted from memory):

| Source | License | Clean-room status | Role |
|---|---|---|---|
| ares | ISC | readable | reference-accuracy emulator; oracle + trace hooks |
| paraLLEl-RDP | MIT | readable (its Angrylion reference lineage is unlicensed — excluded) | LLE RDP candidate for U6 |
| n64-systemtest | MIT | readable + vendorable | CPU/COP1/RSP/RDP/TLB/exception conformance ROMs (self-checking; real-hardware provenance of expected values is unverified either way) |
| libdragon | Unlicense | readable | probe-ROM authoring; endorses ares for validation |
| MAME | GPL-2.0+ whole | source excluded; documented Lua/debugger interfaces usable black-box | secondary tracer at best — its own N64 driver is flagged `MACHINE_NOT_WORKING` |
| angr | BSD-2 | readable | MIPS64-BE VSA/symbolic reference (R4300-specific fidelity unverified) |
| ddisasm | AGPL-3.0 | concepts-only (paper, never code) | published validation of the monotonic-fact-DB disassembly architecture |

Answer-key corpus expansion, graded by artifact quality and license: 
Banjo-Kazooie (CC0, 100% complete, `symbol_addrs.*.txt`) and Perfect Dark
(MIT, ~97.5%, `symbol_addrs.*.txt`) are clean direct-hit keys; Super Mario
64 (CC0, 100%) needs linker-map parsing; Diddy Kong Racing (CC0, ~97.75%)
is a strong alternate. Paper Mario and Majora's Mask have the best splat
tooling but **no license**; GoldenEye is ranked last (89.1%, no license,
active rights disputes around the title). Keys require the user's own ROMs to
grade against; ingestion tooling ships with loud env-declared skips.

The rights check those three were held for was resolved by project-owner
decision 2026-08-01 (AGENTS.md "Clean-room protocol"): measured observations
about a ROM — addresses, segment maps, overlay geometry, symbol-to-address
bindings — may be read from a decompilation project whatever its declared
license, because they are facts about the cartridge that any disassembly
reproduces, not the project's expression. Copying or adapting such a
project's *code* remains disallowed.

### Observation intake

| Source | License | Date | What was taken |
|---|---|---|---|
| pmret/papermario | none declared | 2026-08-01 | `ver/pal/splat.yaml` segment map for the PAL ROM (sha1 `2111d392…`, matching its `checksum.sha1`): 687 kseg0 ROM→VRAM segment mappings, read as measurements to explain an overlay-recovery failure. No source read into fn64. |
| bomberhackers/bm64 | none declared | 2026-08-01 | `splat.yaml` segment map only. **Targets a different revision** — sha1 `8f9e1706…`, a 16 MB build whose overlays begin at ROM `0x800000`, while the local ROM is 8 MB (sha1 `8a7648d8…`), so its addresses do not transfer and were not used as such. Read for engine *structure*, which is revision-stable. No source read into fn64. |

That measurement disproved a standing hypothesis and identified the real
cause: Paper Mario maps 687 segments onto only 35 distinct VRAM destinations
(420 segments share `0x80240000`) with 683 distinct
`vram - rom_offset` deltas, and every VRAM lands below `0x8080_0000`. So
`SearchConfig::aki_family`'s `vram_hi` bound was never the blocker, and
`delta_vote`'s search for a dominant shared delta is structurally
inapplicable to an overlay-swapping engine: the descriptor's own `vram_dest`
is the authority there, and corroboration has to come from the loaded image
decoding as MIPS at that VA rather than from agreement between sibling
regions. 195 of 687 regions also fall below `aki_family`'s `min_region_len`
of `0x1000` (`vrom_family` already relaxes this to `0x80`).

Bomberman 64 sharpens why the remaining zero-candidate ROMs stay open. Its
decomp shows the same swapping shape — 111 overlay segments onto only four
VRAM destinations, 95 sub-overlays sharing `0x80043000` — with every region
inside `aki_family`'s size bounds. What differs is where the metadata lives:
each overlay carries its **own** `ovl_N_header`, so there is no central
`(rom_start, rom_end, vram_dest)` array for `enumerate_family_tables` to walk.
These ROMs are not missing overlays, they are missing a *table*. Recovering
per-overlay headers is a distinct detector from family-table enumeration, and
is not attempted here.

Corrections measured during intake (2026-07-18, same day): Perfect Dark has
NO splat `symbol_addrs` table at its repo root — the survey's claim was
falsified by direct fetch (its symbols live in `ld/*.inc` for an armips
build; map/linker-script extraction is the follow-up). Banjo-Kazooie's root
`symbol_addrs.us.v10.txt` is a 60-row hand-maintained override list, not the
full per-function boundary table; it is vendored with provenance
(`testdata/answer_keys/LICENSES.md`) and parsed by `gate_keys`, but full
Banjo boundary ground truth also needs deeper extraction. ares v148 has no
headless video mode, no CLI trace toggle, no savestate-save trigger, and no
input-replay subsystem (verified by reading its ISC source), and its
first-launch Gatekeeper prompt blocked sandboxed execution entirely, so
n64-systemtest results under ares remain uncollected; the DEBUGGER=1
mupen64plus core stays the working automation vehicle.

Dynamic-tracing tooling decision (2026-07-18, research-backed). Two findings
settle the emulator question: (1) **no headless or file-tracing ares exists** —
verified from the installed v148 binary (only display/settings CLI flags, no
`--headless`/`--trace`, monolithic GUI) and from a fork survey (the best fork,
`HailToDodongo/ares-64`, ISC, adds only a GUI debugger — no trace-to-file, CLI,
Lua, or GDB; ares's sole programmatic surface is a GDB DebugServer). (2) **The
N64 decomp community does not trace execution at all** — Zelda/MM/SM64 decomp
is static byte-for-byte matching (Splat + matching-decomp tools + asm-differ +
decomp-permuter +
objdiff); indirect targets and boundaries are resolved statically with manual
jump-table annotation, not by dynamic PC capture. fn64's dynamic-trace ambition
is therefore closer to a recompilation workflow than a decompilation one — novel
capability, not a missing standard tool. Consequences: static discovery remains
the main line (where all measured leverage is), and ares is parked as an
accuracy oracle reachable only via its GDB stub, not a bulk tracer. BizHawk's
mupen core has `ITraceable` file tracing but is the same slow interpreter
class and pulls GPL, so it is not preferred.

**Retraction (2026-07-25):** this section originally recommended an
"x86_64/dynarec rebuild ... under Rosetta" of the custom DEBUGGER=1 mupen
core, on the premise that "the arm64 build is `NO_ASM=1` pure-interpreter"
because "mupen's Makefile only disables the dynarec for arm64." That premise
is now obsolete and the remedy was never the right one. A native Apple
Silicon (darwin-arm64) dynarec was added upstream (in flight as PR #1184; see
the pin in `tools/mupen-trace/README.md` and `build-core.sh`), so the arm64
build is no longer forced to `NO_ASM`. But this doesn't touch the actual
bottleneck: the debugger path used for single-stepping is interpreter-speed
on *any* architecture, because `EnableDebugger` requires `R4300Emulator=0` —
mupen64plus disables the dynarec whenever the debugger is armed, x86_64 and
arm64 alike. An x86_64/Rosetta build would not have fixed this; it would
still run the interpreter while debugging, just on emulated x86_64 instead of
native arm64 — strictly worse, not better. The conclusion "the debugger-driven
trace path is slow" therefore survives, but both the stated cause and the
Rosetta remedy were wrong. The real fix that shipped instead: a full-speed
capture mode (`FN64_CAPTURE_SECONDS`, `tools/mupen-trace/mupen_devtrace.c`)
that skips the debugger entirely and drives the core-side PI DMA emitter at
native arm64 dynarec speed, trading single-step determinism for wall-clock
throughput (measured: minutes of single-stepped emulated time collapse to
seconds). It is not deterministic for every ROM (measured, not assumed — SM64
was byte-identical across runs, GoldenEye and Perfect Dark were not), so it
complements rather than replaces the debugger path.

## WM2000 (NWXE) whole-ROM recompile — milestone status (2026-07-18)

The origin ROM, pointed at by the full discovery stack. `gate_wm2000_recompile`
derives the five materialized banks, constructs the ROM-bound topology and
backed dense generation catalog, runs
`compose_catalog_bound_direct_transfer_fixed_point_v1`, and passes that exact
validated result to schema-v2 block-pack emission, closure scoring, and source-
frontier reporting.
Cross-bank authority now includes both proven direct calls and computed calls
whose source plus delay slot are proven code and whose one exhaustive typed
analysis exactly matches the CFG target set. Bounded/open calls, computed
jumps, and mismatched evidence remain non-authoritative. Exact cross-call
reachability advances to a monotone fixed point only when an exact target VA
identifies one target generation. An overlapping VA confers neither
reachability nor semantic callback-argument authority until a typed activation-
compatibility capability identifies the executable generation. Locally
contained targets remain suppressed from sibling projection, unique authority
records remain capped and canonically ordered, and delay-slot root validation
runs after every authority rebuild and at finalization. Duplicate input bank
names fail before preparation because bank names key the fixed-point state. The
2026-07-31 ROM-bearing result is recorded below; older large-denominator pack
counts remain retained only as historical schema context.

Retained historical whole-ROM pack: **38,194 blocks / 205,086 words / 820,344
emitted bytes** (portable pack JSON 8.45 MB, no ROM instruction words in it).
Retained
historical whole-ROM execution-closure classification (union across all 5
banks, 19,909 reachable
destinations): exact_aot 349 / block_aot 17,642 / dynamic_mips 1,898 /
**unsupported 20** — composing the overlays did NOT raise the unsupported
count; it remained a 20-destination punch-list, and dynamic_mips (1,898,
`dev-interpreter`-coverable) absorbed the AKI dynamic-dispatch sites.
Snapshot schema v2 postdates this measurement. These values remain the last
retained baseline pending a ROM-bearing regeneration; they are not a claim
about current HEAD and no address artifact has been invented from them.

The former `boot:0x800f8e90` blocker is closed. A computed-jump target had
admitted the delay word at `0x800f8e94` as a separate CFG leader, severing the
predecessor pair. Exact calls, jumps, branches, and exhaustive computed
transfers to an ordinary delay word now use a typed delay-entry alias instead:
the predecessor pair stays intact and direct entry executes the shared word
before continuing. Only call-derived aliases gain callable authority.
Snapshot composition still removes candidate-only delay roots while retaining
their facts, and rejects authoritative control-shaped delay entries. A
control-shaped overlap reached only by broad candidate traversal may remain as
diagnostic CFG metadata, but authority projection cannot admit or emit it.
Because an ordinary alias shares its first word with the predecessor block, it
is deliberately `block_aot`, not a separate contiguous `exact_aot` owner; the
generated runner can still dispatch at that exact word and execute it once
before its continuation. Authority projection requires an exact source,
destination, and transfer-kind edge match
except for a consecutive plain fallthrough wholly inside one authority block's
ordinary prefix. That narrow refinement cannot cross a control/terminal
boundary, and bank-end fallthrough is instead represented by an exact typed
`ran_off_end_fallthrough` authority edge. Focused regressions cover both
structural boundaries. The 30-test focused delay suite and the generated-runner
direct-entry regression each passed 10/10 consecutive single-job runs on
2026-07-30. A fresh WM2000 regeneration kept the sound scoreboard unchanged at
1,810 `block_aot`, 13 `dynamic_mips`, and 0 `unsupported`, while correcting
`0x8013b744` from an invented four-byte delay-entry block plus owner-missing
ambiguity to the preserved predecessor pair at `0x8013b740`; it remains dynamic
only because the overlapping generation lacks activation authority.
`gate_wm2000_recompile` now emits all five materialized CPU
bank runners, invokes `rustc`, constructs one `BlockProgram`, and successfully
runs first/middle arbitrary-PC plus typed hole/unaligned probes for every bank.
The 2026-07-31 catalog-fixed-point regeneration reports 2,047 `block_aot`, 19
`dynamic_mips`, and 0 `unsupported`. One catalog-bound call admits overlay 2,
expanding the concrete denominator and exposing a second round of transfers;
the fixed point then stops with no new authorized capabilities. This is a
compiling and executing whole-ROM CPU-runner milestone, not source-closure
authority. Successful dense-fetch confinement does not prove every executable
source, generation activation, writer channel, or possible path has been
enumerated.

Honest scope: this is the CPU-recompilation milestone (all discovered code
packed, classified, emitted, compiled, and mechanically probed). It is NOT a
full-static-closure claim or a booting game — the unsupported/source/writer
frontiers above remain, and RSP audio and RDP graphics are separate U6 runtime
subsystems.

# Phase unlock ledger

Every ROM run must report physical, logical, executable, owner, and function
coverage separately. Precision/recall alone is not ROM coverage.

| ROM / phase | Proven mapped banks | Proven executable bytes | Exact owners | Function-entry precision | Function-entry recall | Open indirect sites |
|---|---:|---:|---:|---:|---:|---:|
| OoT / D1 candidate union | 469 load images | measured by coverage report | not admitted by D1 | 99.5672% | 72.3312% | 548 calls, 38 jumps |
| NW4E / D1 candidate union | 5 overlays + resident | measured by coverage report | not admitted by D1 | 48.4387% | 89.7384% | 14 calls, 65 jumps |
| NWXE / D1 candidate union | boot + discovered images | measured by coverage report | not admitted by D1 | 36.3969% | 28.5422% | 32 calls, 76 jumps |

The NW4E exploratory CFG pass now seeds candidate/supported entries without
promoting them to owner proof. On the hand-fixed 36-rung measurement it moved
from `0/36` recovered to `13/36` exact, `16/36` partial, and `7/36`
unrecovered. It also exposed 39 exploratory overlaps; those remain a
diagnostic and the exact-owner gate still rejects them. This is the intended
separation between coverage exploration and proof-qualified admission.

Exact owners are no longer zero. With reached-proven-code executable
derivation in `ProgramSnapshotV1` composition, the `gate_b2` snapshots admit
20 of 27 NWXE boot-bank assessments (4,156 reached executable bytes in 24
intervals) and 32 of 45 OoT boot-bank assessments (6,744 reached executable
bytes in 35 intervals), every admitted extent agreeing with its answer key
under a hard `wrong=0` gate. What remains blocked is typed:
`trailing_unattributed_code` and `interior_candidate_entry` boundary
attribution, non-contiguous owners, and the malformed/unproven-word cases the
NWXE histogram above enumerates. NW4E still admits no exact owner — its resident
grading path runs the exploratory CFG, not snapshot composition.

The table is a reporting contract, not a static answer key. Gate binaries must
emit the same fields for every phase so that a new rule can be evaluated by
the physical bytes and bank identities it unlocks, not just by candidate
counts. A phase may increase function recall while reducing precision; that
is progress only when the newly reached bytes are either proven or explicitly
left in the unresolved frontier.

Static NW4E selector evidence is recorded in `aki_reference::NW4E_SELECTOR`
and mechanically re-derived by `gate_selector`. The dispatcher is VA
`0x80026888` (ROM `0x27488`); an earlier record said `0x80027488` because it
assumed a `VA = ROM + 0x8000_0000` resident delta, which contradicts the
byte-verified boot facts (header entry `0x80000400`, IPL3 copy source ROM
`0x1000`, so the delta is `0x7fff_f400`). The correction was disambiguated
mapping-independently: all twelve absolute `jal` targets inside the
dispatcher land on `addiu $sp,$sp,-N` prologues only under the corrected
delta. The dispatcher reads the flag word at `0x800a10b0` with branch masks
`0x1` (skip R2), `0x8` (skip R3), `0x40` (take R5), loop mask `0x2`, loads R4
after the flag-controlled loop, and re-loads R1 every iteration. The masks
establish control-flow predicates only; they do not assert which runtime
states set the flag.

`gate_selector` additionally establishes, over the NW4E ROM:

- The dispatcher zero-initializes the flag itself (`sw $zero` at
  `0x800268f0`) and writes a companion mode byte at `0x80097fd8` with
  per-branch constants (0 init/R3, 3 R2, 2 R5, 1 after the loop).
- No `j`/`jal`/branch in any canonical bank targets the dispatcher. Its
  entry is data-derived: the wrapper at `0x80026830` materializes the
  dispatcher address into `$a2` and passes it to the thread create/start
  pair (`0x80037520`/`0x800376e0`).
- A linear HI/LO cross-reference sweep (`xref::scan_global_refs`, candidate
  evidence only) finds exactly eight flag stores: the resident init plus
  seven overlay stores — R2 `0x80106940` (linear value `0x22`), R2
  `0x80106dac` (join-dependent, reported unresolved, byte-inspected values
  `0x2`/`0xe`), R2 `0x80106dec` (zero), R3 `0x80109124`/`0x80109140` (value
  `0x1`), R3 `0x80109178` (switch-tail join; linear fall-through `0x12`,
  byte-inspected case values `0x2,0x3,0x6,0x12,0x18,0x22,0x40`), and R5
  `0x80106824` (value `0x3`). R1 and R4 contain no flag references.
- All five descriptor-record pointers (record base + `0x10`) are
  materialized once each inside the dispatcher, matching
  `NW4E_DESCRIPTOR_TABLE` geometry and the R1,R2,R3,R4,R5 record order.

Ten consecutive `gate_selector` runs produced identical output, SHA-256
`b53b25c7dd0a92dda59182f78f5c3ac0e0147124ea19941516da92a391679290`,
re-checkable via `scripts/gate-determinism.sh`.

Black-box emulator corroboration (2026-07-18): Mupen64Plus 2.6.0
(`--nosaveoptions --sshotdir <tmp> --testshots 60,120,180,240`, Rice video,
HLE RSP, pure-interpreter core) boots the same NW4E ROM and rendered four
verified non-blank frames (per-channel stdev 44-99, hundreds-to-thousands of
unique colors; legal-disclaimer and THQ/JAKKS logo screens inspected
visually), so the boot path the selector dispatcher belongs to demonstrably
runs. The selector flag word itself was NOT observable: the Homebrew-bottled
core rejects `--debug` ("can't use --debug feature with this Mupen64Plus
core library" — the tool's own black-box error), and `--help` documents no
other memory-inspection interface. No selector-state coverage is claimed;
runtime flag observation remains a stage-4 trace-ingestion frontier and
needs either a debugger-enabled emulator build or the project's own
headless trace producer. Captures live outside the repository. Overlay
store sites are candidate cross-references on proven load-image bytes;
executable permission and natural reachability of those sites remain open.

The debugger-enabled follow-up (2026-07-18, same day) closed the tooling
half of that frontier: mupen64plus-core tag 2.6.0 (commit `b0d68c2`) built
from source with `DEBUGGER=1 NO_ASM=1` accepts `--debug`, and a small driver
against the publicly documented `m64p_debug` API (dlopen/dlsym, no static
linking, no GPL implementation source read) read live RDRAM at the flag and
mode-byte addresses during NW4E boot, deterministically across ten runs.
Steady-state observations: flag `0x0` (the dispatcher's zero-init) and mode
byte `0x00` then `0x03` — both inside the statically predicted sets, with
the mode sequence matching the documented R2-branch value. One transient
out-of-set word (`0x20004002` ≈ 2 ms after interpreter start) decodes as a
MIPS instruction and precedes any plausible dispatcher execution, so it is
attributed to the boot-stage segment copy transiting that address, not a
flag store. Deep-boot flag transitions were NOT reached: the debugger session
runs as a pure interpreter, too slow to leave the logo screens within the
observed budget. This is a `DEBUGGER=1` constraint, not an arm64 one — mupen's
`EnableDebugger` requires `R4300Emulator=0` on every host architecture, so an
x86_64/Rosetta build would not have helped (see the retraction under
"Dynamic-tracing tooling decision" above). Open next steps: a longer
unattended run, a full-speed capture that skips the debugger
(`FN64_CAPTURE_SECONDS`, see above), or a `DebugBreakpointCommand` write
watchpoint instead of polling. No selector-state coverage is claimed beyond
the values actually observed.

An aligned-pointer-run experiment (four or more words targeting one load image)
was measured and rejected from the canonical harvest: it produced only 3.10%
precision on NW4E and 2.34% on NWXE. Pointer runs remain exploratory until
conditioned on stronger table-shape and code-target evidence.
