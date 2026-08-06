# The checkpoint digest is the throughput blocker

Decision document. Records what was measured, why the obvious fix is not
available as a performance change, and what a migration would have to cover.
It does not recommend acting — the choice is a certification decision.

Measured 2026-08-06 on WM2000, `examples/wm2000-block-boot`, Apple Silicon.

## The measurement

Release build with `-C force-frame-pointers=yes` and `debug = 1` scoped to the
three handwritten crates (`fn64-abi`, `fn64-runtime`, `fn64-recomp-rs`) in
`examples/wm2000-block-boot/Cargo.toml`, leaving the generated shards at
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
