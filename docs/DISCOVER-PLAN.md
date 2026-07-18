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
| NWXE, boot mapping only | 36.3969% | 28.5422% | overlays are absent |
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

The byte-verified `ProgramSnapshotV1` now closes the native resident-bank
passes into one artifact. With only the NWXE header entry as a traversal seed,
the real-ROM gate produces 197 blocks, 27 owner assessments, zero partition
ambiguities/overlaps, and 26 exact + 3 coarse answer-key grading matches with
zero wrong splits. Ten complete compositions serialize byte-identically. Its
owner frontier is more useful than a blended score:

```text
not_proven_executable   27 assessments (sole blocker for 25)
owner_not_contiguous     2 assessments
malformed_block          1 assessment / 31 sites
word_not_proven_code     1 assessment / 9 sites
```

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
`5944f1a0c63523591cbef33c4856c594b2cca38466945bc63da35a7459dace44` (no test
re-checks this digest until ROADMAP H3 makes `gate_b2`'s compile-time input
paths reproducible), and compiles the generated runner with `rustc`. Addresses in gaps receive no
dispatch arm; static and computed transfers into them remain unresolved.

This orders the next work. First recover proof-carrying resident executable
regions; it can admit 25 otherwise-closed owners immediately.
Then replace the assumption that a function is one contiguous all-code byte
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
The emitter supplies the registration helper and its compile/run gate enters
an interior PC through that program. Live executor/shell ownership remains
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
re-checkable via `scripts/gate-determinism.sh`. Overlay
store sites are candidate cross-references on proven load-image bytes;
executable permission and natural reachability of those sites remain open.

An aligned-pointer-run experiment (four or more words targeting one load image)
was measured and rejected from the canonical harvest: it produced only 3.10%
precision on NW4E and 2.34% on NWXE. Pointer runs remain exploratory until
conditioned on stronger table-shape and code-target evidence.
