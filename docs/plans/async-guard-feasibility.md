# Async guard / observation worker: feasibility

Investigation document. Asks whether host-side observation work — the mutation
journal, its digests, and a hypothetical execution recorder — can move off the
executor thread onto a worker.

**Verdict: the A/B split is real and is cleanly drawn, but the offload is not
worth building, because the work it would move costs approximately nothing.
Measured, the entire category-B apparatus is 0 ms of a 430 ms run.**

Measured 2026-08-07 on WM2000, `recomps/wm2000/packages/wm2000-block-boot`, Apple Silicon, at
`a867dba` — i.e. *after* the page-tree digest migration, the `mprotect` write
barrier, and the two barrier refinements (`c9db8a3`, `976e7a4`).

This matters, and is the single most important thing in this document: **the
70%-sha2 profile that motivated the question is from before those landed.**

## The measurement that settles it

`FN64_FAST_MUTATION_JOURNAL=1` disables exactly the continuous snapshot and
journal — category B in full (`live_program.rs:22-33`). It is therefore the
**upper bound on any possible offload win**: a perfect worker thread that did
category-B work for free could not beat deleting it.

Lane, 8 runs each, 200,000-step cap (run ends at 19,523 steps on thread death):

| lane | wall |
|---|---|
| guard on (`FN64_MPROTECT_BARRIER=1`) | 0.43 0.43 0.44 0.43 0.43 0.43 0.43 0.43 |
| category B deleted (`+FN64_FAST_MUTATION_JOURNAL=1`) | 0.43 0.43 0.43 0.43 0.43 0.44 0.43 0.43 |

**Identical.** Both lanes produce `sim_time=13990253`, `steps=19523`,
`run_queue=[6, 1]` — determinism preserved, and the two lanes are
indistinguishable in wall clock.

The theoretical maximum win from offloading category B is **zero, within the
noise floor of the measurement.** A worker thread cannot beat deletion, and
deletion buys nothing.

For scale, the barrier that replaced the old full-region scan is worth 6.5x:

| lane | wall |
|---|---|
| barrier off (full 1.44 MiB scan per boundary) | 2.78 2.78 2.80 |
| barrier on | 0.42 0.42 0.41 |

That 2.78 → 0.42 is the win that was already taken. It is why the remaining
guard cost is unmeasurable: the expensive part is gone.

### The counters, not the profile

As requested — a counter, not a sampler.

`FN64_MPROTECT_BARRIER_SYSCALLS=1` over the same run:

```
[mprotect-syscalls] total=46494 (21.0ms)
  arm=1 (0.0ms, 6667ns each)       disarm=1 (0.0ms, 4083ns each)
  reprotect=23246 (12.1ms, 521ns)  fault=23246 (8.9ms, 383ns each)
```

`FN64_MPROTECT_BARRIER_STATS=1`:

```
[mprotect-barrier] boundaries=91446 served=91445 (100.00%) fell_back=1 (0.00%)
                   clean=68615 (75.03%) mean_dirty_pages_per_served=0.2532
```

**21 ms of syscall time in a 430 ms run — 4.9%** — and that is the *barrier*
(category A infrastructure that must stay synchronous), not the journal. The
0.2532 figure the coordinator cited reproduces exactly on my run.

The `sample` profiler, 228 total samples on the main thread, attributes 11 to
`sha2::compress`. At 228 samples a count of 11 has a ~±3 Poisson error and
translates to roughly **4.8% ± 1.4%** — consistent with the A/B result of "not
measurable", and far from the 70.30% the pre-migration document records. Both
methods agree that sha2 is now small. **I am not claiming 4.8% is the win; the
A/B says the win is 0%, and the A/B is the more trustworthy instrument** because
it measures wall clock on the same binary rather than attributing samples.

## 1. Is the A/B split real?

**Yes, and more cleanly than the brief supposed — the gate does not read the
journal's digests at all.** They are two independent SHA-256 computations over
the same bytes.

### Category A — the gate, synchronous by necessity

`activate_for_fetch_with_digest` (`crates/fn64-recomp-rs/src/generation/mod.rs:771`)
computes a digest from **live memory** via `live_sha256_with`
(`generation/mod.rs:1453-1474`) and compares it to the generation's *own*
`expected_sha256` — a constant baked into the generation record at compile
time, **not** the journal's running `expected_sha256`.

The decision it makes is `AotMiss` vs. execute (`generation/mod.rs:817-836`).
That is a data dependency: deferring it means stale code already ran. Category A
is immovable, exactly as the brief states. Its runtime callers:

- `crates/fn64-abi/src/recompiled/runners.rs:67, 289, 309, 330, 388, 424, 1068, 1081`
- `crates/fn64-abi/src/recompiled/snapshots.rs:1539`
- `crates/fn64-abi/src/recompiled/live_program.rs:2066-2075`

### Category B — the journal, write-only during the run

Every runtime consumer of the journal's digests, traced:

| site | what it does | reads a digest to branch? |
|---|---|---|
| `live_program.rs:535-545` `seal_with` | writes `expected_sha256`, `journal_root_sha256` | no — pure write |
| `live_program.rs:817` `adopt_snapshot` | writes `expected_sha256` | no |
| `live_program.rs:841` `adopt_from_view` | writes `expected_sha256` | no |
| `live_program.rs:965-1020` `commit_snapshot` | reads `before_sha256` **only to store it into the entry it is writing** | no |
| `live_program.rs:1016` | chains `journal_root_sha256` into the next root | no — producer |
| `live_program.rs:1040-1042` `evidence_snapshot` | copies both out for receipts | no — after the fact |
| `snapshots.rs:766, 787` | folds the root into telemetry | no |

`commit_snapshot` is the only site that *reads* `expected_sha256` mid-run, and
it reads it as the `before_sha256` **field value** of the journal entry it is
constructing. No control flow depends on it. Confirmed by the profile: 100% of
sha2 samples land in `digest_expected` (`live_program.rs:512`, the root
recompute) and `adopt_changed_from_view` (`live_program.rs:911`, the leaf
rehash) — both pure producers.

The remaining `expected_sha256` readers are all validation/receipt paths that
run after the fact (`validation.rs:511-1660`, `receipts.rs:1427-1762`).

**So category B is genuinely write-only during a run.** The split is real. It
is simply not worth exploiting.

## 2. The delta handoff — the mechanism exists, and it is already used

The coordinator's correction is right on both counts, and worth recording
because it dissolves the crux rather than solving it.

The barrier already produces the delta: `dirty_spans()`
(`crates/fn64-abi/src/write_barrier.rs:1230`) returns `Vec<(u32, u32)>`, and it
is **consuming** — one window per read, which is what makes a missed `arm` cost
a scan instead of corrupting the guard (`write_barrier.rs:1235-1242`).

And the digest is already delta-shaped: the v2 page tree rehashes only the
pages `set_expected` refreshed (`live_program.rs:500-519`).

**So there is no 1.44 MB copy to avoid — the copy is already gone.** The crux
the brief posed was resolved by work that has already landed. That is precisely
why the A/B measures zero.

### But the root is not delta-shaped, and this is the part to be honest about

`watched_root_digest_v2` (`receipts.rs:1317-1334`) hashes **every page digest
in every range**, on every commit:

```
370 pages × 32 bytes = 11,840 bytes of root message per commit
```

against 4,096 bytes for a single leaf rehash. **The root costs ~2.9x the leaf
it was introduced to avoid.** So the remaining sha2 is dominated by the root
recompute, not the leaf — consistent with the profile, where `digest_expected`
takes 9 of the 11 sha2 samples and `adopt_changed_from_view` takes 5.

This is a real inefficiency and it has a known fix that does **not** need a
thread: make the root an actual tree (hash pairs, cache interior nodes) so a
one-leaf change costs log₂(370) ≈ 9 node hashes instead of 370. That would be a
strictly larger win than any threading scheme, at a fraction of the complexity.

**It is still not worth doing**, because the whole of category B measures at
zero. Recorded here so it is not re-derived.

## 3. Can category A be pipelined or precomputed?

Not usefully, and speculative verification would make things worse.

- The gate is already rare. It runs at *activation*, not per dispatch: the run
  has 91,446 boundaries but activation happens a handful of times (this route
  reports one digest-selected generation).
- Speculating "which generation activates next" requires predicting guest
  control flow. A wrong guess costs a full `live_sha256_with` over the
  candidate's image for nothing.
- A correct guess must still be **revalidated** at the fetch, because the guest
  may have written the bytes in between. The barrier tells you *whether* to
  revalidate, but that check is already what the gate does. There is no saving.

The one sound precomputation — caching "these bytes have not changed since the
last digest" — is exactly `guest_write_token`, which
`snapshots.rs:1229-1231` records as deliberately having **no non-test
consumers** so that no activation path can bypass the digest. Wiring it in is
explicitly ruled out by `9fc0e37` ("guest_write_token must not be wired into
activation"). Not reopening that.

## 4. Recording — the genuinely offloadable case, and its real cost

The coordinator is right that recording separates cleanly from digesting: a log
of `(address, old_bytes, new_bytes)` is self-contained, needs no access to live
RDRAM, and races with nothing.

**But the bytes do not exist today, and capturing them is new hot-path work.**

What exists is spans (`dirty_spans()` → `(u32, u32)`). What a replay/audit log
needs additionally is the *contents*. The costs, quantified:

- **New bytes** are cheap: they are live in RDRAM at the boundary, and the
  changed extent is small (the `0x9b0b3` corruption was 4 bytes).
- **Old bytes** are the problem. At fault time the baseline is still in
  `range.expected`, so they are readable — but the fault handler
  (`write_barrier.rs:557`) is documented as doing *nothing but* set a bitmap bit
  and call `mprotect`: no allocation, no `RefCell`, no lock, no formatting, no
  libc. Copying bytes out of it violates that contract, and the handler runs
  23,246 times per run.
- Capturing at the **boundary** instead of the fault avoids the handler, but by
  then the guest has already overwritten the old bytes — so old-vs-new must be
  diffed against `expected`, which is what `changed_ranges_from_view` already
  does. That path is 14 of 228 samples (~6%), the second-largest handwritten
  cost in the profile.

So recording is *architecturally* offloadable — the log is self-contained and a
worker could consume it with no synchronization. The obstacle is not the worker;
it is that **producing the log is itself hot-path work that does not exist
today**, on the order of the cost of the thing being offloaded. The channel
send (a `Vec` push plus an atomic, per boundary, 91,446 times) is comparable to
the 21 ms the entire barrier syscall load costs.

If full-fidelity recording at real time becomes a product requirement, this is
the design — capture at the boundary, diff against `expected`, ship
`(span, old, new)` over a channel — and it is tractable. It is just not free,
and nothing in the current run needs it.

## 5. What it would cost, and what could break

For the record, had the win been real:

1. **First real concurrency in a deliberately single-threaded design.**
   `crates/fn64-runtime/src/thread.rs:1-19` makes "two coroutines executing
   concurrently" *structurally unrepresentable* via `RunToken`'s private
   constructor. A worker does not violate that directly (it runs no guest code),
   but it ends the property that there is exactly one thread touching runtime
   state, which is the invariant that makes the current design auditable.
2. **Ordering is load-bearing.** `journal_root_sha256` is a **chain**:
   `entry.journal_root_sha256 = canonical_mutation_entry_root(self.journal_root_sha256, &entry)`
   (`live_program.rs:1016`). Entries must be folded in sequence. A worker must
   therefore process a strictly ordered queue — which means it cannot go wider
   than one thread, and any backpressure stalls the executor at exactly the
   point the parallelism was supposed to help.
3. **The root has a read-ordering dependency.** Answering the coordinator's Q3
   directly: the root is not read mid-run for control flow (§1), but it *is*
   read at every commit as the chain input for the next entry. So a worker that
   is behind must be waited for before the next commit can fold — turning the
   handoff into a synchronization point rather than a parallelization, unless
   the executor keeps its own running root, at which point the worker is not
   doing the work.
4. **`RefCell`/`thread_local` throughout.** `PENDING_ATTRIBUTED_EXECUTABLE_WRITES`
   (`live_program.rs:1044`) and the recycled-buffer pool
   (`self.recycled.borrow_mut()`) are thread-local by construction. Crossing a
   thread means re-homing all of it.
5. **The guard's value is its immediacy.** It caught two real corruptions today
   plus two bugs in the write barrier during its own development. A deferred
   guard reports a corruption N boundaries after the fact, when the executor has
   already run on bad state — strictly weaker, which the brief forbids.

## Recommendation

**Do not build the worker.** Not because it is unsound — the A/B split is real
and category B is genuinely write-only — but because the work it would move
measures at zero against an upper-bound experiment that deletes that work
entirely.

If the digest ever becomes hot again (a title with a much larger watched region,
or many more commits), the ordered fix list is:

1. Make `watched_root_digest_v2` an actual tree — ~370 hashes → ~9 per commit,
   no thread, no new concurrency. **But note this redefines a certified digest**
   and carries the same schema-migration cost `checkpoint-digest-cost.md`
   documents: versioned schema, full receipt regeneration, gate expectation
   updates.
2. Only then consider a worker, and only for recording, which is the one case
   with a self-contained handoff.

## Corrections to prior documents

`docs/plans/checkpoint-digest-cost.md` reports sha2 at **70.30%** of self time
and the run at 36.5s/60k steps. Both are pre-migration. At `a867dba` the same
route runs in **0.43s** and sha2 is at or below ~5%, with the A/B saying the
journal's true marginal cost is unmeasurable. That document's central claim —
"the checkpoint digest is the throughput blocker" — **is no longer true**, and
its authorization for the page-tree migration has already been acted on. Anyone
reading it for current numbers will be misled by an order of magnitude.
