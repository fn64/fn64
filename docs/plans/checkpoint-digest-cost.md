# The checkpoint digest is the throughput blocker

> **STALE — 2026-08-07. Do not read the numbers below as current.**
>
> This document reports SHA-256 at 70.30% of self time and the 60k benchmark at
> 36.5 s. Both predate the v2 page-tree migration and the `mprotect` write
> barrier. **The same deep route now runs in ~0.43 s** — the headline claim is
> false by more than an order of magnitude.
>
> What remains true: the *mechanism* it describes, and the reasoning about why
> software substitutes for the guard must read the region. For current numbers
> see `docs/plans/resolvable-self-time-profile.md`, which supersedes the profile
> here.
>
> A stale profile in this file steered at least two optimization waves at the
> wrong target. Date and supersede performance documents.


Decision document. Records what was measured, why the obvious fix is not
available as a performance change, and what a migration would have to cover.
It does not recommend acting — the choice is a certification decision.

Measured 2026-08-06 on WM2000, `recomps/wm2000/packages/wm2000-block-boot`, Apple Silicon.

## The measurement

Release build with `-C force-frame-pointers=yes` and `debug = 1` scoped to the
three handwritten crates (`fn64-abi`, `fn64-runtime`, `fn64-recomp-rs`) in
`recomps/wm2000/packages/wm2000-block-boot/Cargo.toml`, leaving the generated shards at
`debug = false`. Without those line tables `run_one_step` inlines its whole
callee tree and a sampler attributes ~92% of self time to one frame that
cannot be broken down.

`sample`, 20s at 1ms. **Self time = sample count minus immediate children**,
not inclusive totals — reading inclusive numbers as self time caused three
consecutive failed optimizations earlier in this repo's history.

Self time, 15,864 root samples, at 200,000 steps:

| SELF% | samples | symbol |
|---|---|---|
| **70.30%** | 11152 | `sha2::sha256::aarch64::compress` |
| 6.63% | 1051 | `_platform_memcmp` |
| 6.24% | 990 | `_platform_memmove` |
| 5.45% | 864 | `current_changed_ranges` (`live_program.rs`) |
| 3.98% | 631 | `WatchedExecutableBytesV1::set_expected` |
| 3.86% | 612 | `RdramView::copy_logical_bytes` (`rdram.rs:383`) |
| 2.70% | 428 | `madvise` (allocator churn) |
| **0.06%** | 9 | `wm2000_block_shard_00::runner_00::run_00` — **the actual guest code** |
| 0.03% | 4 | `fn64_abi::pi::timing::advance_device_time` |

The stack:

```
run_catalog_block_program                13485
  +-- commit_snapshot                    11153   live_program.rs:638
        +-- digest_snapshot              11152
              +-- sha2 compress          11152
```

Coroutine switching, executor thread selection and `advance_device_time` are
each at or below 0.1%. There is no distributed cost to chase: it is one call.

## Corroboration from the opposite direction

A yield-reason census over the 60,000-step benchmark:

```
[yield-census] total=60000 {"InstructionCheckpoint": 60000}
```

**100% of slice ends are checkpoint publications** — never budget exhaustion,
device access, host shims, or message-queue operations. Every dispatch ends by
publishing a checkpoint, and every publication that changed anything re-hashes
the full 1.14 MiB watched region.

That also explains a negative result: raising `FN64_BLOCK_INSTRUCTION_BUDGET`
from 4096 to 65536 produced byte-identical `sim_time` and identical wall time.
The slice never ends on budget, so a larger budget cannot amortize anything.

## Scale

A step advances **3 sim cycles** (`sim_time=180000` for `steps=60000`), so the
gap to a 93.75 MHz N64 CPU is roughly **31,000x**, and the recompiled guest
code is ~0.06% of where the time goes. The recompiler is not the bottleneck;
the evidence machinery wrapped around it is.

## Why this is not fixable as a performance change

The natural fix is a page-tree digest: hash fixed-size pages, keep the per-page
digests, and on a change re-hash only the affected pages. That changes the
digest's **value**, and the value is load-bearing:

- `expected_sha256` feeds `journal_root_sha256` through
  `canonical_mutation_initial_root` / `canonical_mutation_entry_root`.
- It is cross-checked against `watched_bytes_sha256`
  (`crates/fn64-abi/src/recompiled/receipts.rs:1252`), an independent flat
  SHA-256 over the same watched bytes, at bootstrap validation.
- `final_watched_sha256` appears in eight receipt schemas
  (`receipts.rs:116-483`).

So redefining it changes every certified receipt value, including the
byte-exact rebuild proofs. That is a schema migration, not an optimization,
and it is why this was recorded rather than acted on.

SHA-256 offers no shortcut that preserves the value: it is sequential over the
whole message, so an edit in the middle forces a rehash from that point. There
is no incremental update that yields the same digest.

## What a migration would have to cover

1. A **versioned** digest schema, so existing receipts remain verifiable under
   v1 while new runs emit v2 — the two cannot silently coexist under one name.
2. Regeneration of every committed receipt and expected-closure fixture that
   embeds a watched digest.
3. A decision on `watched_bytes_sha256`, which is deliberately an *independent*
   implementation of the same quantity. If it adopts the page tree too, it
   stops being an independent check; if it does not, the two disagree by
   construction and the bootstrap cross-check has to be restated.
4. Gate updates: `scripts/grade-all.sh` and the byte-exact rebuild proofs
   compare against stored digests.
5. A statement of what the page tree still guarantees. The flat digest binds
   the whole region in one value; a tree binds it through a root, which is
   equivalent only if the tree structure is itself covered by the root.

## What was done instead

Sound, value-preserving changes only, none of which alter any digest:

- `matches_view` / `matches_storage`: decide "did anything change" with one
  `memcmp` per watched range against a pre-reversed baseline mirror, instead of
  allocating 1.14 MiB, copying it, word-reversing 262,144 words, and only then
  comparing. Applied at the dispatch reconcile and at the child-writer commit
  (`execution.rs`), which previously lacked the short-circuit.
- Buffer recycling in `read_snapshot_from_view`, removing a 1.14 MiB
  allocate-and-free from the snapshot path.

These remove the copy, compare and allocation costs around the digest. They do
not touch the digest, which is why the benchmark barely moves: at 60,000 steps
the run is **36.6s before and 36.5s after**, within noise. Reported as a
negative result on speed. Their value is that they isolate the remaining cost
to `digest_snapshot` unambiguously, which is what makes this document's
conclusion falsifiable rather than inferred.

## Hypotheses falsified along the way

Recorded so they are not re-proposed:

| hypothesis | measurement that killed it |
|---|---|
| The journal snapshot is ~100% of runtime | journal on 36.6s vs off 31.0s at 60k — ~15% |
| The per-dispatch scheduler mirror dominates | gating it out entirely: 38s -> 37s, ~3% |
| A larger per-dispatch instruction budget amortizes overhead | 4096 vs 65536: byte-identical `sim_time`, identical wall time |

## Decision (2026-08-06, from the project owner)

**Sequence: finish the dispatch-granularity investigation first, then do the
page-tree digest migration.**

The migration is AUTHORIZED, including the consequence that every certified
evidence value changes and the receipt chain is regenerated. It is gated behind
`docs/plans/dispatch-granularity.md` for one reason: the digest is 70% of self
time, but removing it entirely still leaves ~5,700x, because a dispatch
advances only 3 guest cycles. Until that residual is understood, we cannot say
whether the digest is the dominant lever or merely the largest visible one --
and churning every receipt in the project before knowing that is the wrong
order of operations.

What the migration will need when it runs, unchanged from the analysis above:
a versioned schema bump so old and new digests are distinguishable rather than
silently incompatible, regeneration of the receipt chain and gate expectations,
and an explicit note that digests recorded in existing docs are historical.

The `FN64_FAST_MUTATION_JOURNAL=1` opt-out remains the iteration lane in the
meantime; it does not affect certified runs.


## Superseded in part (2026-08-06, later the same day)

The dispatch-granularity investigation (`docs/plans/dispatch-granularity.md`,
commit `8d85748`) found the residual this document deferred to, and it changes
which fix should go first.

**100% of slices (60,000/60,000) end on `BlockExit::ExecutableWrite` -- a guest
store. Mean block length is 2.0 instructions; mean slice is 3.0 instructions
across 1.5 blocks.**

`EXECUTABLE_WRITE_RANGES` holds the entire 1 MiB boot bank -- the same region
this document's digest hashes -- so `classify_live_executable_write`
(`recompiled/snapshots.rs:983`) cannot tell a store to an ordinary guest
variable from self-modifying code. Every store ends the block, refuses to
chain, publishes a checkpoint, and round-trips the scheduler. The chaining
machinery already works: the same census recorded 29,999 chained `Transfer`
exits.

So the two costs share one root cause -- an over-broad watched region -- but
they are not equally expensive to fix:

- The page-tree digest migration **necessarily redefines a hashed quantity**,
  requiring a versioned schema and full receipt regeneration.
- Narrowing the write-boundary predicate **redefines no hash**. It changes a
  scheduling grain. `sim_time` totals are invariant to slice grouping
  (`executor/mod.rs:1642-1650` accumulates retired instructions); what moves is
  when timers and VI retraces are evaluated.

**Revised recommendation: measure the boundary narrowing before committing to
the migration's scope.** It is correctness-sensitive -- a predicate that is too
permissive lets stale translated code execute -- so it needs care, but it costs
no evidence values.

One correction to this document's own analysis: the claim that SHA-256 offers
no value-preserving shortcut is too strong. It holds for an edit before the
end, but a strict UNCHANGED PREFIX is exactly the exception, and
`sha2::Sha256: Clone` exposes the mid-stream state. A prefix cache is
implemented and proven bit-identical over all 64 change-subsets of a five-range
watched set; whether the prefix actually holds on WM2000 is unmeasured, and if
the boot range is the one that changes it is a negative result.

## The prefix cache is a NEGATIVE result (2026-08-06, measured)

The open question above is now settled by measurement, and the answer is no.

A census inside `digest_snapshot`, over the standard 60,000-step benchmark and
again at 200,000 steps, reports the watched geometry and which range moves:

```
60k:   calls=53248  ranges=2 absorbed=80,567,205,920  skipped=851,936    first_changed{0=1,1=53246,15=1}
200k: calls=167936  ranges=2 absorbed=254,096,572,512 skipped=2,686,880  first_changed{0=4,1=167931,15=1}
```

Three facts, and each alone is fatal:

1. **WM2000 watches TWO ranges, not a boot bank plus a list of overlay slots.**
   This document and the plan built on it both assumed the latter.
2. **The megabyte is the LAST range, and it is the one that changes.** Range 0
   is 16 bytes; range 1 is 1,513,056 bytes (1.44 MiB), and range 1 is what
   differs on 53,246 of 53,248 digests at 60k and 167,931 of 167,936 at 200k.
3. **So the usable prefix is 16 bytes.** The cache skips 0.001% of absorbed
   bytes at both step counts.

Wall clock confirms it: **36.52s at 60k**, against the 36.5-36.6s this document
records at HEAD -- unchanged, within noise. `sim_time=180000` exactly, and the
200k run reproduced its own counters (`sim_time=1461877`, `thread0_dead=true`).

The mechanism is sound and the implementation was proven bit-identical over all
64 change-subsets of a five-range watched set. It simply has nothing to work
with here: only a strict prefix is resumable, and the changing range is last.
The implementation was reverted rather than carried as dead weight.

**This strengthens the case for narrowing the watched region rather than
optimizing the hash over it.** All three costs -- the digest, the store-per-
block dispatch grain, and this dead prefix -- trace to one 1.44 MiB range that
is both watched and constantly written. A page tree would fix the digest by
redefining it; narrowing the region fixes all three and redefines nothing.
Re-examine whether that 1.44 MiB genuinely needs to be watched as one unit
before paying for the schema migration.

## The coalescing hypothesis is FALSIFIED (2026-08-06, measured)

The paragraph above proposes narrowing the region as the fix that "redefines
nothing". Both halves of that are wrong, and both were settled by measurement
rather than argument. This is the fourth hypothesis to die on this question.

### The merge is not what widens the range

The suspicion was that `executable_physical_ranges_for_parts`
(`crates/fn64-abi/src/recompiled/receipts.rs:1108-1156`) coalesces with
`start <= previous.1`, merging *adjacent* spans and not merely overlapping
ones, and that the 1.44 MiB is therefore a code span glued to a data span.

A probe printing the pre- and post-merge sets on WM2000 says otherwise:

```
[watched-probe] pre-merge count=35   (32 distinct after dedupe)
[watched-probe] post-merge count=2
[watched-probe]   post 0x00000180..0x00000190 len=16
[watched-probe]   post 0x00000400..0x00171a60 len=1513056
```

The installed set (`EXECUTABLE_WRITE_RANGES`, `execution.rs:190`) matches the
post-merge set exactly, so this is the set the boundary predicate consults.

Recomputing the merge with strict overlap (`start < previous.1`) yields **17
ranges covering 1,513,072 bytes — the identical byte set**. Not one byte fewer
is watched. The only hole anywhere in the union is `0x190..0x400`, between the
exception vector and the boot bank.

**The spans are genuinely contiguous.** Changing the comparison operator is a
pure no-op on which bytes are watched, and would buy nothing.

### What the 1.44 MiB actually is

Breaking the pre-merge spans down by provenance:

- **31 distinct virtual banks**, nearly all exactly 16,384 words (64 KiB),
  tiling `0x400..0x161460` back to back — `0x400`, `0x10400`, `0x20400`, …
  Each is a *declared executable code bank*, not data that drifted in.
- **One generation invalidation range** `0xe1b90..0x171a60` (589,520 bytes),
  of which 522,448 bytes are already covered by those banks and 67,072 bytes
  extend past `0x161460`.

So the watched megabyte is not one over-broad span. It is 31 declared code
banks that happen to abut. There is no code/data seam to cut along, and
"narrowing the region" has no cheap version: every byte in it is claimed by
something that declared itself executable.

### Splitting DOES change certified digests

Even setting the above aside, splitting is not free. Four hashed quantities
absorb per-range framing, so re-partitioning the same bytes changes the
message and therefore the digest:

| quantity | site | what it absorbs |
|---|---|---|
| `expected_sha256` | `live_program.rs:368` `digest_snapshot` | `start`,`end`,bytes **per range** |
| `watched_bytes_sha256` | `receipts.rs:1252` | `start`,`end`,bytes **per range** |
| `journal_root_sha256` | `receipts.rs:1356` `canonical_mutation_initial_root` | `start`,`end` **per range** |
| `bootstrap_receipt_sha256` | `receipts.rs:1371` | `watched_ranges.len()` **and** each `start`,`end` |

Splitting `[A,C)` into `[A,B)+[B,C)` turns the hashed message from
`A|C|bytes` into `A|B|bytes₁|B|C|bytes₂` — 8 extra framing bytes, different
digest, identical memory. `bootstrap_receipt_sha256` hashes the range *count*
outright, so 2 → 17 moves it directly.

**This is the same certification cost as the page-tree migration**: versioned
schema, full receipt regeneration, gate expectation updates. It is not the
cheap bookkeeping change the paragraph above assumed.

### The no-op-store idea is also a negative result

Separately proposed: have `classify_live_executable_write` compare the stored
value against the sealed baseline and return `Continue` when a store writes
bytes already present. This redefines no hash, so it looked cheap.

It does not pay. The BSS-clear destination `[0x4b4c0, 0xb1390)` is **not zero
in the baseline** — the IPL3 boot DMA copied 1 MiB of ROM there, including
real instruction words (`0x60000` holds `27bdffd8 afb00010`, a MIPS prologue).
Over the 104,372 words the loop clears:

```
nonzero baseline words: 98493  (94.4%)
already-zero words:      5879   (5.6%)
```

**94.4% of those stores genuinely change a byte.** A slice would still break
every ~1.06 words. Note also that the boundary observer is documented
post-commit (`host.rs:188`), so the comparison must be against the sealed
`expected` baseline, not live RDRAM — comparing live memory to itself would
always report "unchanged" and silently disable the guard.

### Where this leaves the decision

Both "cheap" alternatives to the page-tree migration are now closed:

- Splitting the coalesced range changes the same certified surface the
  migration does, and there is no code/data seam to split along anyway.
- The no-op-store filter changes no hash but recovers 5.6% of stores.

The remaining lever on the dispatch grain is still the one
`dispatch-granularity.md` identifies — a resident-code range set distinct from
the watched set, consulted only by the boundary predicate. That leaves all
four digests untouched because it does not repartition the watched set; it
adds a second, narrower set. Its cost is the correctness proof that the
narrower set never misses a genuine self-modifying store, plus the timing
granularity change on publication digests. Nothing measured here makes that
cheaper, but nothing measured here rules it out either.

## The narrow boundary map is ruled out: the stores land on real code

Measured 2026-08-06, same lane. The separate-boundary-map plan requires a
conservative narrower set — one that excludes only spans which cannot back
resident translated code. On WM2000 no such exclusion exists for the stores
that actually end the slices, so the set cannot be narrowed at all.

The slice-ending stores were attributed by PC census at 200,000 steps:

```
site ExecutableWrite pc=0x80027154 count=93596
site ExecutableWrite pc=0x80000414 count=52186
site ExecutableWrite pc=0x80000418 count=52186
```

Both dominant sites are data-clearing loops, disassembled from the ROM through
`ROM_COPY = (0x1000, 0x101000, 0x80000400)`:

```
80000400  lui   $t0, 0x8005
80000404  addiu $t0, $t0, -0x4b40   ; dest  = 0x8004b4c0
80000408  lui   $t1, 0x0006
8000040c  addiu $t1, $t1, 0x5ed0    ; count = 0x65ed0 = 417,488
80000410  sw    $zero, 0($t0)
80000414  sw    $zero, 4($t0)
80000418  addi  $t0, $t0, 8
8000041c  addi  $t1, $t1, -8
80000420  bne   $t1, $zero, 0x80000410
```

The destination is **not** the code the loop is running from. It clears
`0x8004b4c0..0x800b1390`, and that interval is covered by declared, compiled
boot shards:

| shard va_start | overlap | share of shard |
|---|---|---|
| 0x80040400 | 20,288 | 31% |
| 0x80050400 | 65,536 | 100% |
| 0x80060400 | 65,536 | 100% |
| 0x80070400 | 65,536 | 100% |
| 0x80080400 | 65,536 | 100% |
| 0x80090400 | 65,536 | 100% |
| 0x800a0400 | 65,536 | 100% |
| 0x800b0400 | 3,984 | 6% |

417,488 bytes — seven shards wholly inside the cleared region, two partly.
These are not padding swept in by 64 KiB tiling. Shard 05
(`0x80050400..0x80060400`, entirely inside the clear) carries 33 emitted
functions and its `WORDS` are dense MIPS: `0x27BDFFF8` (`addiu $sp,$sp,-8`),
`0xAFB00000` (`sw $s0,0($sp)`), `0x03E00008` (`jr $ra`). Sampling the ROM
image across the destination shows ordinary prologues throughout, e.g.
`27bdffd8 afb00010 00808021` at `0x80060000`.

So the boot stub really does overwrite bytes that back resident translated
code, and it does so 52,186 times in the censused window. A boundary map that
excluded those spans would be excluding live code — the exact one-sided
failure the plan forbids, where stale translated code executes. A map that
includes them is the map we already have.

This is a property of the program, not of how the range set is represented:
the game's BSS/heap overlaps its own loaded code image, and the recompiler
declared shards over the whole copied 1 MiB because the ROM copy is one
contiguous blob. No authority — generation catalog or AOT extents — can
certify those spans dead, because they are not dead; they are code that has
been compiled and whose bytes are being zeroed before reuse as data.

The 5.6% no-op-store measurement above is consistent and independent: 94.4% of
the cleared words genuinely change value, so the writes are real mutations of
real code bytes.

### Why this closes the line rather than redirecting it

Nothing narrower is available, and the payoff would have been small in any
case. Slice granularity is not what costs the time — the profile above puts
70.3% of self time in `sha2::compress` under `digest_snapshot`. Lengthening
slices reduces the *number* of publications, but each publication still
re-hashes the full watched region, and the census shows the run already
reaches 410-519-block slices wherever the guest does not store. The dispatch
grain is a symptom of the digest cost, not an independent blocker.

Baseline for the record, 60k steps: 32.98s, `sim_time=180000` (invariant, as
required). Census at 200k: `slices=199751 instructions_per_slice=7.163
blocks_per_slice=2.108`, exit mix `{ExecutableWrite: 199292, HostCall: 272,
Checkpoint: 177, ExecutableWriteResolveCall: 9, ThreadReturn: 1}`.

No source change is carried for this entry — the probe used to obtain the
range decomposition was reverted, and the boundary map was never written,
because the derivation it depends on does not exist on this program.

Verification for this entry: 691/691 `fn64-abi`+`fn64-runtime`, 401/401
`fn64-recomp-rs`, `grade-all.sh` wrong=0 on all five, 60k benchmark
`sim_time=180000` at 36.16s (baseline ~36.5s). The probe was reverted; no
source change is carried. (`fn64-discover` shows 1068/1069 — the OoT
`auto_strategy_corpus` failure is pre-existing on this branch and unrelated:
that crate does not depend on `fn64-abi`.)

## Done (2026-08-06): the page-tree digest migration, v1 -> v2

The migration this document authorized is implemented and measured. The digest
went from 70.3% of self time to **2.3%**, and the 60k benchmark from **32.71s
to 11.29s (2.90x)**.

### What the digest is now

`digest_snapshot` no longer hashes the watched bytes flat. Each watched range is
partitioned into **4096-byte pages**; each page has its own SHA-256 leaf, and
the root hashes the leaves.

- Leaf: `"fn64.canonical-watched-bytes-digest.v2" || 0x00 || page_bytes ||
  physical_start || physical_end || page_index || page_len || bytes`
- Root: `"fn64.canonical-watched-bytes-digest.v2" || 0x01 || page_bytes ||
  range_count || (physical_start || physical_end || page_count || leaves...)*`

The leaf binds the range bounds and the page index, so a page cannot be replayed
at a different address or a different position in the range; the leaf binds its
own length, so a short final page cannot be confused with a zero-padded full
one; the root binds the range count and each range's page count, so no
regrouping of pages produces the same root. Distinct leaf and root tags mean a
leaf can never be read as a root.

**Page size = 4096, why.** The cost is bounded on both sides and flat between.
Below ~1 KiB the per-page fixed cost (a `Sha256::new`, a 38-byte prefix, a
`finalize` -- about 2 compression blocks) stops being amortized, AND the root
grows, because the root hashes 32 bytes per page and is recomputed on *every*
commit. Above ~16 KiB a single-word guest store re-hashes more than it must. At
4 KiB, WM2000's 1,513,056-byte range is 370 pages: leaf overhead is ~3%, the
root hashes 11,840 bytes, and a one-store commit hashes one 4 KiB page plus that
root instead of 1.44 MiB.

Measured on the 60k route: **55,759 page rehashes across 55,505 commits** --
1.005 pages per commit. The incremental path does what it was designed to do.

### v1 values are historical

Every digest value recorded in this document and in
`docs/plans/dispatch-granularity.md` prior to this section is a **v1 flat
digest** and is not reproducible under v2. They are retained as the record of
what was measured, not as expectations. No v1 value should be compared against
a v2 run.

`watched_bytes_sha256` (`receipts.rs`) **deliberately stays v1 and stays flat.**
It is the independent cross-check of the bootstrap watched bytes, it runs once,
and it is not on the hot path. Point 3 of the migration checklist above asked
for a decision on it: the decision is that it remains an independent
implementation, which is the only thing that makes the bootstrap cross-check
worth having. Making it a second page-tree would have made the two agree by
construction rather than by evidence.

### Correction to this document's regeneration estimate

The checklist above (points 2 and 4) anticipated regenerating committed receipt
values and gate expectations. **That work did not exist.** An exhaustive search
for 64-hex-character literals over the whole tree found **zero** hardcoded
digest expectations over watched executable memory -- `crates/fn64-abi`, which
owns the entire chain, contains no such literal in any file, source or test. The
chain is computed-and-cross-checked end to end: every assertion compares a
recomputed value against a carried one, never against a constant.

Consequently the migration required **no fixture, gate, test, or reference-TSV
edits at all**. The claim at point 4 that `scripts/grade-all.sh` "compares
against stored digests" was also wrong: that script grades `fn64-discover`
symbol recovery and contains no digest expectation. The digests in
`scripts/gate-determinism.sh` are discovery-gate JSON outputs, unrelated to
watched memory.

This is worth recording as a property of the design, not luck: because the
receipt chain never pinned a literal, a schema migration of the digest cost
three source files and no regeneration.

### Determinism

`page_tree_root_is_independent_of_incremental_history`
(`crates/fn64-abi/src/recompiled/tests/mutation_state.rs`) drives a long-lived
state through 13 commits over two watched ranges -- one sub-page, one spanning
several pages and ending mid-page -- touching page first bytes, page last bytes,
spans straddling boundaries, whole pages, the short final page, and pages
already dirtied. After every commit it requires the incrementally maintained
root to equal (a) the from-scratch root of the same bytes and (b) the root of a
**fresh state with no history**, sealed directly on those bytes. It then
restores the original bytes and requires the original root back, so the cache
holds no residue of the path taken.

The test was verified to fail: skipping every third dirty page makes it fail at
round 2.

Dirty tracking is decided in `refresh_page_digests` by `memcmp` of the incoming
page against the current baseline, at the moment of the update. A page is
skipped only when its bytes are *equal*, and equal bytes have an equal digest.
Nothing else can cause a skip -- not a writer's declaration, not
`current_changed_ranges`, not a flag maintained elsewhere -- so a changed page
keeping a stale digest is not representable.

### Guards unchanged

No guard weakened. `current_changed_ranges`, `first_uncovered_changed_range`,
`matches_view`, the pending-write quiescence assertions and the poison path are
untouched; the mutation journal still detects any undeclared change to watched
executable memory. `commit_snapshot` adopts the baseline slightly earlier so the
entry can be built from the incremental root -- after every check that inspects
the old baseline has already run.

### What is the bottleneck now

Not the digest. Self time at 400k steps, after:

| SELF | symbol |
|---|---|
| 2757 | `_platform_memcmp` |
| 1456 | `_platform_memmove` |
| 1029 | `current_changed_ranges` |
| 920 | `RdramView::copy_logical_bytes` |
| 915 | `WatchedExecutableBytesV1::set_expected` |
| **173** | `sha2::sha256::aarch64::compress` |

The remaining cost is the snapshot machinery -- copying and comparing 1.44 MiB
per commit -- which is the over-broad watched region that
`docs/plans/dispatch-granularity.md` identifies. Narrowing
`EXECUTABLE_WRITE_RANGES` is now the next lever, and unlike this migration it
redefines no hashed quantity.

## The snapshot no longer materializes (2026-08-06)

The bottleneck above was removed without narrowing the watched region at all.

`read_snapshot_from_view` allocated a `Vec<u8>` per watched range and copied
the whole 1.44 MiB out of RDRAM -- with a per-word byte-lane reversal -- before
anything looked at it. Its consumers then mostly just **compared** it:
`current_changed_ranges` against `expected`, and `commit_snapshot` updating
`expected` from it. The copy existed only so the comparison had a contiguous
logical-order buffer.

`matches_view` / `matches_storage` already decided the **boolean** form of that
comparison in one `memcmp` per range, against the pre-reversed
`expected_storage_order` mirror. Two new methods carry that same shape further:

- `WatchedExecutableBytesV1::changed_ranges_into` -- the same comparison,
  carried far enough to name the differing bytes. The word-aligned body is one
  `memcmp` against the mirror, chunked so equal stretches are skipped 256 words
  at a time; only differing words are walked lane by lane. The at-most-three
  head and tail bytes stay on `read_u8`.
- `WatchedExecutableBytesV1::apply_changed_from_view` -- refreshes `expected`,
  `expected_storage_order` and the page digests over **only** the changed
  ranges, instead of rewriting all three over the whole region.

### The lane mapping

Logical byte `a` lives at storage `a ^ 3`, and across an aligned word that XOR
*is* the little-endian reversal. So inside a differing word, storage lane `k`
is logical lane `3 - k`. Getting this wrong reports "unchanged" for changed
memory only for particular lane patterns -- a silent corruption this repository
has already paid for once -- so it is proven rather than argued.

`changed_ranges_from_view_matches_the_copying_path` drives randomized contents
and randomized change patterns (five densities, from "nothing" to "every byte")
over nine watched layouts: unaligned start, unaligned end, both, sub-word
ranges entirely inside one storage word, ranges shorter than the 3-byte head,
ranges straddling word boundaries, and a three-range watched set. Each round it
asserts the new comparison returns **exactly** the ranges
`current_changed_ranges(&read_snapshot_from_view(v))` returns, then commits
through both paths and asserts `expected`, `expected_storage_order`, the page
digests, the watched root and the journal root all stay byte-identical.
`adopt_from_view_matches_the_copying_adoption` does the same for the
no-declaration path.

### The `expected` baseline invariant

`expected` ends byte-identical to what the full-copy path produces. Bytes
inside the changed ranges are re-read through `copy_logical_bytes` itself -- the
same function the full copy used, so the same lane mapping by construction.
Bytes outside them were proven equal to the baseline by the very comparison
that produced the changed set. The storage-order mirror is widened to whole
words across the body (a partial word cannot be reversed in isolation), which
only re-derives bytes that are already correct.

`refresh_page_digests_over` is as conservative as the form it parallels: the
dirty set comes from `changed`, which is a byte-for-byte comparison of live
storage against the old baseline -- not from a writer's declaration. A page
outside every changed range has identical bytes by that comparison, and
identical bytes have an identical digest.

### Guards unchanged

Nothing weakened. `first_uncovered_changed_range`, the pending-write quiescence
assertions and the poison path are untouched, and they see the same changed
list they saw before. `changed_ranges_from_view` returns `None` when the state
is unsealed or a watched byte is unmapped, and every caller then falls back to
the copying path -- so each panic message is still produced by exactly the code
that produced it before.

### Result

| SELF (before) | symbol | after |
|---|---|---|
| 2757 | `_platform_memcmp` | ~25 |
| 1456 | `_platform_memmove` | not in the top table |
| 1029 | `current_changed_ranges` | not called on the commit path |
| 920 | `RdramView::copy_logical_bytes` | only over changed bytes |
| 915 | `WatchedExecutableBytesV1::set_expected` | not called on the commit path |
| 173 | `sha2::sha256::aarch64::compress` | unchanged |

At 400k steps the sampler now attributes 11,982 of 12,102 root samples to a
single inlined `run_one_step` frame: the snapshot machinery has fallen out of
the profile entirely.

- 60k, `FN64_ABSENT_N64DD=1 FN64_BLOCK_MAX_STEPS=60000`: **11.54s -> 4.60s
  (2.51x)**, `sim_time=180000` both, every progress counter identical.
- 200k route: **42.33s -> 18.98s (2.23x)**, `sim_time=1461877` both,
  `thread0_dead=true` both, counters byte-identical
  (`trace=200221 device_trace=421 pi_started=105`).

Best-of-3 on the same build configuration. Cumulative with the v2 page tree,
the 60k benchmark has gone **32.71s -> 4.60s (7.1x)**.

- `cargo nextest run -p fn64-abi --features recomp-rs -p fn64-runtime`:
  **696/696** (694 pre-existing + 2 new equivalence tests).
- `cargo nextest run -p fn64-recomp-rs`: **401/401**.
- `cargo nextest run -p fn64-discover`: **1069/1069** -- the OoT
  `auto_strategy_corpus` failure noted above did not reproduce on this run.
- `scripts/grade-all.sh`: wrong=0 on all five (nw4e-donor 925, nw4e-solo 873,
  nwxe-donor 779, nwxe-solo 725, revenge-solo 597).

### Verification

- `cargo nextest run -p fn64-abi --features recomp-rs -p fn64-runtime`:
  **694/694** (691 pre-existing + 3 new).
- `cargo nextest run -p fn64-recomp-rs`: **401/401**.
- `cargo nextest run -p fn64-discover`: 1068/1069 -- the OoT
  `auto_strategy_corpus` failure is pre-existing, confirmed by reverting this
  change and re-running that test alone.
- `scripts/grade-all.sh`: wrong=0 on all five configurations.
- 60k, `FN64_ABSENT_N64DD=1 FN64_BLOCK_MAX_STEPS=60000`: **32.71s -> 11.29s**,
  `sim_time=180000` both (invariant, as required), progress counters identical.
- 200k route: **107.51s -> 43.19s (2.49x)**, `sim_time=1461877` both,
  `thread0_dead=true` both, progress counters byte-identical
  (`trace=200221 device_trace=421 pi_started=105`, all others 0).

Timings are best-of-3 on the same build configuration. An earlier
before/after pair in this investigation showed no speedup; that comparison was
invalid -- the two binaries had been built with different
`FN64_EXECUTABLE_IMAGES` environments, so one lane was not running the closed
AOT catalog. Recorded because the null result was believed for a while.

## Done (2026-08-07): the root becomes a tree, v2 -> v3

The v2 migration above made the digest's LEAVES incremental and left its ROOT
flat. This is the other half of it.

### What was left

`watched_root_digest_v2` absorbed 32 bytes of every page digest on every call.
WM2000 watches 371 pages across two ranges, so each commit that changed
anything -- however little -- re-absorbed 11,872 bytes into the root. A
four-byte guest store rehashed one 4 KiB leaf and then the entire leaf vector.

The resolvable self-time profile (`docs/plans/resolvable-self-time-profile.md`,
5 runs, 7,532 samples) put `sha2::sha256::aarch64::compress` at **26.14%** of
steady-state self time, of which **20 of the 26 points were `digest_expected`**
(`live_program.rs`), i.e. the root and not the leaves.

### What the digest is now

Per watched range, a binary Merkle tree over that range's page leaves; then a
small flat root over the range roots. Five tagged message forms, all prefixed
with `fn64.canonical-watched-bytes-digest.v3`:

| tag | form | message after the schema prefix |
|---|---|---|
| `0x00` | leaf | `page_bytes \|\| start \|\| end \|\| page_index \|\| len \|\| bytes` |
| `0x02` | internal pair | `fanout \|\| start \|\| end \|\| height \|\| index \|\| left \|\| right` |
| `0x03` | promoted single child | as above, **without** `right` |
| `0x04` | range root | `page_bytes \|\| fanout \|\| start \|\| end \|\| page_count \|\| present \|\| apex` |
| `0x05` | top root | `page_bytes \|\| fanout \|\| range_count \|\| range_roots...` |

The structure is bound rather than inferred: fanout, page size, each node's
height and index, each range's bounds and page count, and the range count. No
regrouping of pages, levels or ranges reaches the same root.

The odd trailing node is promoted through its **own** message rather than by
hashing `H(x||x)`. Duplication is the classic Merkle malleability -- it lets a
`2n`-leaf tree whose second half repeats its first collide with an `n`-leaf
tree. Distinct arities make that unrepresentable.

**Fanout 2, why.** Bytes hashed per commit go as `f/ln(f) * ln(n)`, minimised at
`f = e`. For WM2000's 370 pages: `f=2` is 9 nodes over 576 payload bytes,
`f=4` is 5 over 640, `f=16` is 3 over 1536. Binary also keeps the incremental
update a plain parent walk with no sibling gather.

The schema strings for v2 and v3 are the same LENGTH, deliberately, so the
version separation rests on the bytes themselves rather than on a length shift.

### The saving is counted, not inferred

A temporary census inside the hash functions, both lanes, deep route
(`FN64_BLOCK_MAX_STEPS=19523 FN64_MPROTECT_BARRIER=1`):

```
v2: leaf calls=17534 bytes=71,795,552 | node bytes=0         | root bytes=186,615,968 | TOTAL 258,411,520
v3: leaf calls=17534 bytes=71,795,552 | node bytes=9,213,376 | root bytes=  1,006,016 | TOTAL  82,014,944
```

Leaf calls and leaf bytes are **byte-identical across the lanes**, which is the
control: only the root changed. Root-side hashing fell **18.3x** (186.6 MB to
10.2 MB) and the total digest payload **3.15x**. The probe was reverted.

### Measured, interleaved -- CORRECTED

The first measurement of this change was taken on a CONTENDED machine and its
magnitude was wrong. Both numbers are kept, because the correction is the
more useful record.

**Contended (load ~20, 15 competing `rustc`):** v2 773.5 ms, v3 726.3 ms,
paired median delta 54.9 ms, 15/15 positive.

**Quiet (load < 2, no competing processes), 15 interleaved pairs:**

| lane | median | min | sd |
|---|---|---|---|
| v2 flat root | 421.5 ms | 416.9 | 3.4 |
| v3 Merkle root | 392.3 ms | 384.8 | 3.4 |

**Paired delta: median 30.3 ms, 15/15 positive, 1.074x.**

Two things follow, and the second is the lesson.

First, the quiet baseline **reproduces the documented 420-440 ms exactly**
(421.5 ms). The 775 ms seen during development was entirely machine
contention, not drift in the program.

Second, **the interleaved paired design preserved the SIGN and the
consistency but NOT the magnitude.** 15/15 positive was true in both
conditions; 54.9 ms was inflated ~80% over the real 30.3 ms. Standard
deviation tells the story: 22.1/12.9 ms contended against 3.4/3.4 ms quiet.
Interleaving defends against drift and ordering, not against a noise floor
six times the effect. A paired result on a loaded machine may be reported as
directional, never as a magnitude.

### The digest share did NOT fall, and that is the real finding

Profiled with `recomps/wm2000/scripts/profile-wm2000-self-time.zsh`, 5 runs per lane, same
quiet machine, same session:

| lane | `sha2::compress` SELF | total weighted cycles |
|---|---|---|
| v2 | 33.50% | 7.232 G |
| v3 | **34.05%** | 7.245 G |

The share is FLAT, and the totals are indistinguishable -- while the counted
payload fell 3.15x. These are not contradictory once the census is read
carefully, and the resolution matters for what to do next:

- The payload that fell is the ROOT term, 186.6 MB to 10.2 MB.
- The LEAF term did not move at all: 71,795,552 bytes on both lanes, byte for
  byte. It is now **87.5% of all remaining digest payload.**
- SHA-256 cost is not linear in payload alone -- it is dominated by
  per-invocation setup at small message sizes. The root went from ONE call
  absorbing 11,872 bytes to 9 node calls absorbing 64 bytes each. Fewer bytes,
  but ~9x the `Sha256::new`/`finalize` pairs, and at 64 bytes of payload a
  compression call is nearly all overhead.

So v3 bought 30.3 ms of real wall clock while leaving the sha2 SHARE flat,
because it traded a large-message hash for several small-message hashes. The
saving is real and it is measured; the mechanism is not "less hashing" so much
as "less memory traffic through the hasher".

**This closes the root as a lever and re-opens the leaf.** The next lever on
the digest is the PAGE SIZE, and the argument in
`CANONICAL_WATCHED_BYTES_PAGE_BYTES_V2`'s doc comment is now partly stale: it
warns that shrinking pages inflates the root, which was true when the root was
O(pages) but is not true under v3, where the root is O(log pages). Smaller
pages would cut the 71.8 MB leaf term on single-store commits roughly in
proportion. That is a v4 schema change and should be measured, not assumed --
the counter-pressure is the same per-invocation overhead that just showed up in
the root.

### Equivalence

Complete run output `diff`-identical on both lanes, five paired deep-route runs,
including `sim_time=13990253`, `thread0_dead=true`, every device counter and
every event registration. Separately, over the long route
(`FN64_BLOCK_MAX_STEPS=40000000 FN64_BLOCK_MIN_GUEST_INSTRUCTIONS=1461877`),
both lanes report `achieved_guest_instructions=1461883 scheduler_steps=874
sim_time=1492883` and the identical
`logical_rdram_sha256=3514f2a1e2bf9b667f0a7d0d5bbd1370c85276c49864cb337e9be220aad22080` (no test owns it).
Reproducing that digest needs the ROM and a 40M-step run, so it is the record
of a measurement rather than a gate.

### Determinism, proven against injected faults

`page_tree_root_is_independent_of_incremental_history` is extended three ways,
each because the v2 form was too weak for v3:

1. **Deeper geometry.** The large range goes from 4 leaves to 22, whose levels
   are 22, 11, 6, 3, 2, 1 -- odd at three heights, so the promoted-odd-node path
   is reached at more than one level. The widths are asserted outright so the
   fixture cannot silently flatten and keep passing.
2. **Both commit paths.** The test drove only `commit_snapshot`
   (`refresh_page_digests`, dirty set by page comparison). The path the emulator
   actually takes is `adopt_from_view` (`refresh_page_digests_over`, dirty set
   from the changed-range list). **Measured: skipping every third dirty leaf in
   `refresh_page_digests_over` left the test green** until a second,
   RDRAM-backed state was added to drive both paths over the same edits. The
   equivalence tests did catch it, but only on 1 KiB storage -- a single-leaf
   tree, which could never have caught a multi-level ancestor bug.
3. **Node-by-node comparison.** A stale INTERNAL node is a failure mode v2 did
   not have and is invisible to any check that inspects only leaves or only the
   root. Every level is now compared against a from-scratch build after every
   commit.

`the_v3_messages_are_exactly_what_the_schema_says` pins each level's hashed
message field by field against an independently written reference. It exists
because **`assert_ne!` cannot detect a weakening**: delete the range count from
the top root and every inequality about it still holds, because both sides lost
the same field. Measured -- the structural test stayed green through exactly
that deletion, through a dropped page count, through a reused v2 schema string
and through a colliding tag byte.

Twelve faults injected one at a time, all twelve caught: skipping the ancestor
walk; stopping one level short of the apex; skipping every third dirty leaf on
each of the two commit paths; duplicating the promoted odd child; reading the
apex from the wrong level; omitting a node's height, index or range bounds;
omitting the range root's page count or bounds; omitting the top root's range
count; reusing the v2 schema string in a v3 leaf; and colliding two level tags.

Two faults are correctly NOT failures, recorded so they are not mistaken for
gaps: dropping the ancestor dedupe hashes a parent twice for the same value (a
pure performance regression, no value change), and the single-arity tag and the
child duplication each neutralise the other -- either alone is caught, and so is
the combination.

One methodological note. An early run of the fault matrix reported four faults
as "not caught" that were in fact never applied: `%` in a patch string was eaten
by shell `printf` escaping inside a bash function, so the lanes were identical.
Each fault verdict here comes from a run that asserted the file changed first.

### Regeneration: again, none

Point 2 and 4 of the original checklist anticipated regenerating committed
receipt values and gate expectations. As with v2, **that work did not exist.**
An exhaustive search for 64-hex-character literals finds **zero** hardcoded
digest expectations over watched executable memory anywhere in
`crates/fn64-abi`. The hits elsewhere in the tree are ROM image content hashes
(`recomps/wm2000/packages/wm2000-block-boot`, unchanged by this migration), a NIST SHA-256 test
vector in `recomps/wm2000/packages/wm2000-block-shards/materializer.rs`, and discovery-gate
JSON digests in `scripts/gate-determinism.sh`, none of which concern watched
memory. `scripts/grade-all.sh` grades `fn64-discover` symbol recovery and
contains no digest expectation.

This is the second migration to cost no regeneration, which makes it a property
of the design rather than luck: the receipt chain never pins a literal, so every
assertion compares a recomputed value against a carried one.

### `watched_bytes_sha256` stays v1 and stays flat

The decision is unchanged, and for the same reason. It is the INDEPENDENT
cross-check of the bootstrap watched bytes, it runs once, and it is not on the
hot path. Making it a second Merkle tree would make the two agree by
construction rather than by evidence, and two mechanisms agreeing by
construction is weaker evidence than two agreeing by measurement.

### Guards unchanged

No guard weakened. `current_changed_ranges`, `first_uncovered_changed_range`,
`matches_view`, `matches_storage`, the pending-write quiescence assertions and
the poison path are untouched. The dirty set is still decided by byte
comparison against the old baseline -- never by a writer's declaration -- and
every ancestor of every dirty leaf is recomputed without exception, so a node
whose subtree is clean has identical leaves and therefore an identical value.

The v2 functions are retained under `#[cfg(test)]` as the reference the
version-distinguishability tests hash against. Nothing on a live path computes
them.

### Verification

- `cargo nextest run -p fn64-abi --features recomp-rs -p fn64-runtime`:
  **712/712** (708 pre-existing + 4 new).
- `cargo nextest run -p fn64-recomp-rs`: **401/401**.
- `cargo nextest run -p fn64-discover`: **1069/1069**.
- `scripts/grade-all.sh`: wrong=0 on all five (nw4e-donor 925, nw4e-solo 873,
  nwxe-donor 779, nwxe-solo 725, revenge-solo 597).

### What is the bottleneck now

Not the root. The census says the leaves are 71.8 MB of the 82.0 MB of digest
payload that remain -- 88% -- and they are already incremental at 1.005 rehashes
per commit. The only lever left on the digest itself is the page size, and
`CANONICAL_WATCHED_BYTES_PAGE_BYTES_V2`'s doc comment already argues that 4 KiB
sits in the flat middle of that curve. A meaningful further reduction has to
come from committing less often or watching less memory, and this document
records four separate falsified attempts at the latter.
