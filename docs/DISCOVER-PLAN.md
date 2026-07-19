# Deterministic ROM Discovery Plan

Status: active implementation plan
Last updated: 2026-07-18

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

`gate_d1` grades candidate function starts only. It does not grade extents or
byte ownership.

| Corpus/input | Candidate precision | Entry recall | Important limitation |
|---|---:|---:|---|
| OoT, generalized load tables | 99.5672% | 72.3312% | 469 load images; resident `code`/`n64dd` mapping remains incomplete |
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
`gate_overlay_generalize` is 10/10 byte-identical with the full OoT+GE+PD+SM64
set (SHA-256 `5401e638…`).

**End-to-end payoff — OoT graded with mechanically-recovered overlays**
(`gate_d1_oot_overlays`, held-out, 10/10 byte-identical `ac606195…`). OoT's
existing 99.567%/72.331% grade uses hand-supplied `oot_load_image_tables`
geometry (a per-game input the engine did not infer). Running the identical
function-entry grade through the *mechanically recovered* overlays instead
answers whether automation can replace that hand geometry. The three-way
result: (A) boot-only 62.500%/0.823%; (B) mechanically recovered
**99.567%/72.331%**; (C) hand geometry 99.567%/72.331%. **B now equals C
byte-for-byte — mechanically-recovered overlays reach the hand-supplied
geometry ceiling exactly, at identical precision, recovered from ROM bytes
alone.** This closed in three steps: descriptor-corroborated actor mapping
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
byte-identical throughout. `gate_d1_oot_overlays` is 10/10 byte-identical
(`c8fcb6a1…`); `gate_overlay_generalize` (`dec5742e…`). **This proves the
"port any N64 ROM without per-game hand geometry" thesis on the answer-key
ROM: automation matches the hand-encoded overlay tables, not approximately but
exactly.**

**Execution-closure scoreboard (`gate_closure`, held-out, 10/10 byte-identical
`4ff3a44c…`).** The concrete "distance to a recompilable ROM": every reachable
CPU transfer destination is classified `exact_aot` (inside a proven exact
owner) / `block_aot` (proven reachable code, no source-level owner claimed) /
`dynamic_mips` (open/bounded indirect the interpreter fallback covers) /
`unsupported` (lands outside every known mapping — the release-blocker). Per
ROM (destinations): NW4E block_aot 22,051 / dynamic_mips 892 / **unsupported
11**; NWXE exact_aot 95 / block_aot 17,622 / dynamic_mips 2,169 /
**unsupported 20**; OoT (resident boot bank only — its VROM overlays are
outside snapshot V1's physical-backing composition) block_aot 287 /
dynamic_mips 73 / **unsupported 6**. Held-out grading found
`misclassified_as_code = 0` on all three: no exact_aot/block_aot destination
lands where the dump says data. The headline: the distance to a full-game
build is not thousands of destinations but **6–20 per ROM** — each an
enumerable frontier (boot-DMA/hardware targets and CFG computed-jump
over-approximations). This reframes "recompile it all" from an open-ended
recall question into a small, inspectable punch-list, and confirms the
architecture's design: the large `dynamic_mips` counts (the AKI dynamic-
dispatch `jalr` sites that indirect backward-slicing proved irreducibly static-
open) are fallback-covered, not blockers.

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

`BlockPackV1` now serializes those proven block identities, geometry,
terminators, and per-block digests without ROM words. Materialization
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
| Header/boot | byte-order normalization, header fields, boot copy | proven initial mapping and entry |
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
same way: on this DEBUGGER=1 / pure-interpreter / NO_ASM macOS-arm64 core
build, `M64P_DBG_RUNSTATE_RUNNING` does NOT free-run once a breakpoint is
installed — `sample` backtraces show it parks on a per-instruction semaphore
and only `DebugStep()` advances it, so every instruction still costs one
step round trip. Execution breakpoints at the byte-verified dispatcher PCs
plus a write watchpoint on `0x800a10b0` do fire correctly and
deterministically (4/4 byte-identical runs): the driver reaches the init
store and the R2 flag load with flag/mode at their zero-init values, but a
114M-step / ~10-minute run never reached the R3/R5/loop branches or any
overlay flag store — the interpreter spends that time in a hardware-timing
poll loop. Deep selector-state observation is therefore blocked by
interpreter speed, not driver correctness; the next attempt needs a
dynarec-capable core (x86_64/Rosetta) or a coarser skip mechanism, not more
breakpoints. Traces contain executed-PC sequences from a user's ROM and stay
out of git; compiled drivers are gitignored build artifacts.

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
- Use m2c after boundaries exist; it is not a boundary oracle.
- Port the existing relocation-masked AKI fingerprint and opcode-skeleton
  matching into Rust.
- Require uniqueness, full-body validation, bank compatibility, and a clear
  runner-up margin. A transferred name never proves an extent on its own.

The external-tool path now has two validated producers: spimdisasm
function-info normalization and a synthetic headless Ghidra raw-bank
conformance run. Both emit candidate-only, bank-qualified, digest- and
lineage-bound claims. Ghidra passed ten deterministic runs with same-VA banks
isolated and seeded/unseeded results distinct. The next expansion is not more
function-start voting: export blocks, xrefs, switches, data objects,
prototypes, decompiler types, and stack frames into the canonical graph.

### 8. Pack emission and end-to-end gate

- Emit two views from one fact snapshot: a Recompiler Pack containing only
  admitted banks/owners/transfers, and a Decomp Pack containing matching
  assembly plus provenance-bearing symbols, xrefs, relocations, data objects,
  prototypes/types, stack frames, and Splat/Ghidra/m2c inputs.
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
   unimplemented). The interpreter lane that makes execution closure
   universal: any bank-qualified destination static admission cannot prove
   runs instrumented instead of faulting. With it, "port any ROM" stops
   depending on discovery completeness at all — AOT coverage becomes an
   optimization, and every fallback execution emits promotion traces that
   feed item 1's evidence loop back into AOT admission.
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
| Ghidra conformance | synthetic banks only so far | n/m | n/m | candidate-only |
| Trace producer v1 (500k-step boot window) | n/m | 1,868 executed resident PCs; 639 in proven code, 1,035 candidates corroborated, 194 previously-unclassified; 0 conflicts | n/m | adopted (observed evidence) |
| Trace→FactDb adapter | n/m | ingestion delta 0 → 499,997 facts / 1,868 Supported code-existence conclusions / 478 corroborations / 0 static-data conflicts | n/m | adopted (Supported, distinct evidence class) |
| Delta-voting VA-mapping inference | n/m | 5/5 overlays admitted-correct, 0 open, 0 wrong (margins 3.1x-9.7x) | not graded (no NWXE overlay regions yet) | adopted |
| GP-base voting | n/m | OPEN (0 real $gp constructions; 25 off($gp) = data-as-code noise) | OPEN (7 accesses; unaligned histogram winner rejected) | mechanism adopted; no base to recover on these ROMs |
| Overlay region discovery (descriptor-family search) | n/m | 5/5 regions recovered from ROM alone (table @0x53988 found without being handed it), delta_vote 5/5 correct | **4 overlay regions recovered @table 0x48a68, 100%/100%, delta_vote 4/4 correct, 0 wrong; integrated D1 36.396867%/28.542179% → 49.976448%/86.895987%; a 2nd candidate table correctly rejected** | adopted — mechanically opens NWXE overlays |
| Exact-owner proof on recovered NWXE overlays | n/m | n/m | 6 exact owners (from 0), 0 wrong extents; 22,562 reached blocks, 475,740 proven-executable bytes; dominant blocker unresolved-indirect (614 sole) | adopted — first proof-qualified overlay ownership |
| VROM overlay recovery (file-table resolution) | **OoT: file table @0x7430 recovered (=dmadata); 414 overlay regions, 100% precision / 88.5% recall (actor+kaleido tables admitted)**; SM64 correctly 0 (negative control); GE/PD 0 ungraded | n/m (AKI physical path unchanged) | n/m (unchanged) | adopted — overlay recovery now crosses engine families (AKI + OoT); effect/gamestate tables below 2-region floor stay open |
| OoT end-to-end with recovered overlays (gate_d1_oot_overlays) | **B mechanical NOW EQUALS C hand-geometry EXACTLY: 99.567%/72.331%** (was 48.450%→69.449%→72.331% over 3 steps); all 468 overlay regions recovered (actor 426/426, effect 36/36, gamestate 4/4, kaleido 2/2), 0 wrong | n/m | n/m | thesis proven: mechanical recovery matches hand-encoded overlay geometry exactly, no precision loss, held-out |
| Execution-closure scoreboard (gate_closure) | OoT (boot): block_aot 287, dyn_mips 73, **unsupported 6** | NW4E: block_aot 22051, dyn_mips 892, **unsupported 11** | NWXE: exact 95, block_aot 17622, dyn_mips 2169, **unsupported 20** | adopted — the recompilability metric; 0 misclassified-as-code; distance to full-game build is 6-20 destinations/ROM not thousands |
| Multi-bank cross-overlay owner authority | n/m | n/m | exact_owners 6→7, wrong 0; entry_not_authoritative 987→273 (−714) | adopted — real but exposes partition owner-span construction (owner_missing +578) as next lever |
| Backward-slice indirect resolution (angr pattern, BSD-2) | 1 NW4E site Open→Bounded; precision unchanged | (see NW4E) | wrong 0, all 399 open sites stay open — PROVEN irreducibly static (vtable/return-value jalr = AKI dynamic dispatch) | adopted (sound, robustness) — instrumented negative: 16,366 unresolved_indirect are dynamic_mips territory, not static |
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
tooling but **no license** — symbol-metadata extraction from them is held
until a rights check; GoldenEye is ranked last (89.1%, no license, active
rights disputes around the title). Keys require the user's own ROMs to
grade against; ingestion tooling ships with loud env-declared skips.

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
flag store. Deep-boot flag transitions were NOT reached: the macOS arm64
build runs as a pure interpreter (upstream's own forced `NO_ASM`), too slow
to leave the logo screens within the observed budget. Open next steps:
a longer unattended run, an x86_64/Rosetta core for dynarec speed, or a
`DebugBreakpointCommand` write watchpoint instead of polling. No
selector-state coverage is claimed beyond the values actually observed.

An aligned-pointer-run experiment (four or more words targeting one load image)
was measured and rejected from the canonical harvest: it produced only 3.10%
precision on NW4E and 2.34% on NWXE. Pointer runs remain exploratory until
conditioned on stronger table-shape and code-target evidence.
