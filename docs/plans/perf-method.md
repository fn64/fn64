# Performance method

Written 2026-08-07, after a session that made **nine wrong calls** on this
question and shipped six real wins. The wins all came from handing an agent a
measurement. The wrong calls all came from handing one a hypothesis.

This is not a list of optimizations. It is the procedure that produced the wins
and would have prevented the wrong calls.

## The record this is drawn from

| verdict | claim | reality |
|---|---|---|
| wrong | journal snapshot is ~100% of runtime | ~20% |
| wrong | A/B measured on a clean tree | measured on a peer's uncommitted tree |
| wrong | scheduler mirror call chain is 78% | 3% |
| wrong | a larger instruction budget will amortize dispatch | no effect at all |
| wrong | three targets from a profile | inclusive samples read as self time; all three were artifacts |
| wrong | ~182,892 syscalls, ~316 ms | 206,348 and 231 ms |
| wrong | `guest_write_token` observes RDRAM | it observes *declarations*; caching on it would be unsound |
| wrong | the retirement loop causes the 66.8 GB thrash | `retired_others=0` across all 419,861 activations |
| overstated | v3 digest tree is 54.9 ms | 30.3 ms on a quiet machine |

Six wins landed in the same session: the v2 page-tree digest, in-place watched
comparison, the resident-generation boundary, the mprotect write barrier, the
clean-boundary skip, and selective re-protect. **~19,000x -> ~2.4x hardware.**

## The rules, each earned

### 1. Measure before dispatching, not after
Every one of the nine came from reasoning about code structure. Every win came
from a number. If you cannot state the current cost of the thing you are about
to optimize, you are not ready to optimize it.

### 2. Self time = count minus immediate children
A sampling profiler attributes samples to every frame on the stack. Reading
inclusive totals as self time produced three targets that were **all**
artifacts, including a "symbol" (`live_program::_`) that was a demangled prefix
covering the calls beneath it. `scripts/wm2000_self_time.py` computes this
correctly; use it.

### 3. Count, do not infer
A sampling profiler attributes *samples*, not *calls*. Inferring "N calls at
M ns each" from a profile got both numbers wrong. Twelve `FN64_*_CENSUS` /
`_SYSCALLS` / `_STATS` gates exist in `fn64-abi` for exactly this. Add one
rather than infer.

### 4. Only measure on a quiet machine
`uptime` and `pgrep rustc` before any timing. A concurrent 32-crate shard
rebuild made a 421 ms baseline read 775 ms. `scripts/profile-wm2000-self-time.zsh`
refuses above a load threshold and re-checks *between* runs, because a build
starting mid-profile poisons only the later traces and leaves a plausible
average.

### 5. Interleave A/B pairs, and do not trust magnitude through noise
Not six of lane A then six of lane B — other agents land commits between blocks.
Interleaving preserves the *direction* of an effect through contention but not
its *magnitude*: sd was 22.1 ms contended against 3.4 ms quiet, and a 54.9 ms
reading was really 30.3 ms.

### 6. Prove the lanes differ before believing a number
A fabricated 4.9x came from an env gate where `FN64_MPROTECT_BARRIER=` (empty)
read as ON, so both lanes were the barrier lane. Check a counter or a symbol
that must appear in one lane and not the other. `env_flag` now treats
absent/empty/`0` alike, pinned by a test.

### 7. A line printed on a state CHANGE cannot prove absence of progress
The route was believed to stall at controller read 600, "deterministic across
four runs, three binaries, hours apart." It never stalled. The harness logs only
when scripted input *changes*, and the schedule's last edge is read 600 — so a
healthy run and a wedged one emit byte-identical stdout. The four reproductions
agreed because they were all reading the same schedule file. `sim_time` looked
frozen because two printings of the same log line necessarily carry the same
`sim_time`.

Before calling a long-running process stuck, print on a cadence the *process*
controls — steps or wall clock — never on an event the input script controls.
`FN64_HEARTBEAT=<steps>` does this. This is rule 6 (prove the lanes differ)
pointed at runs instead of lanes.

### 8. Editing `fn64-recomp-rs` costs 32 crate rebuilds
~9-11 minutes, versus ~25 s for `fn64-abi`. Every file in
`crates/fn64-recomp-rs/src` is also a certified source, so an edit changes an
identity digest. Prefer `fn64-abi`; when you must cross, say so in the commit.

### 9. Never run a rebuild-triggering agent beside a benchmarking one
This produced rule 4's phantom. Serialize them.

## Where the cost is, as of `e7c4d04`

Quiet-machine deep route (19,523 steps, `FN64_MPROTECT_BARRIER=1`): **382-392 ms**.

| share | component |
|---|---|
| ~34% | `sha2` — **87.5% of it now leaves**, at 1.005 rehashes/commit |
| ~11% | `RdramView::read_u8` |
| ~11% | `with_executor` |
| ~8% | `changed_ranges_from_view` |
| ~5% | `mprotect` syscalls (was 50.9%) |
| **2.86%** | **the recompiled guest code** |

Category split: per-boundary **~55%**, device timing ~12.5%, per-instruction
~11%.

**The guest code runs at ~0.09x hardware — roughly 11x faster than the console.**
Everything above 1.0x is the correctness apparatus. Codegen is not the lever and
will not be until that 2.86% grows.

## Ranked candidates, with what would falsify each

1. **Page size v4.** The argument against smaller pages in the v2 constant's doc
   comment is **stale**: it warned they inflate an O(pages) root, which the v3
   tree made O(log pages). Leaves are 87.5% of digest payload, so 1-2 KiB pages
   could cut them 2-4x. *Falsified if* per-invocation SHA-256 overhead eats the
   saving — the same effect that limited v3's root win to 30 ms.
2. **Commit frequency.** 15,719 root calls over 19,523 steps: a checkpoint on
   ~80% of steps. *Falsified if* the boundaries are load-bearing.
3. **`digest_expected` allocates a `Vec` per call** — 15,719 allocations, 2
   elements each. Trivial, unmeasured, do not do it without a number.

## Dead ends — do not retry without new evidence

- **Narrowing the watched region.** Four falsified attempts. WM2000 zeroes its
  own loaded code image; compiled shards live inside the memset destination.
- **`verify_precompiled_instruction_word`.** Unfixable at that layer:
  `fn64-recomp-rs` does not depend on `fn64-abi`, so it cannot see the barrier,
  and `verify_live_words` is baked in at shard generation.
- **Caching activation on `guest_write_token`.** Unsound. Its zero-consumer
  property is a *cited premise* of a written safety argument
  (`dispatch-granularity.md:570`).
- **An async guard worker.** Sound but worthless: deleting the whole journal
  (`FN64_FAST_MUTATION_JOURNAL=1`) measures **0 ms**, and a thread cannot beat
  deletion.
- **`codegen-units` / LTO / `target-cpu=native` on the shards.** 10% for a
  9-minute build, or 2.3x *slower* with all three.

## Two things that are not perf but block "playable"

Perf is no longer the binding constraint on the actual goal.

1. ~~**The route stalls at controller read 600.**~~ **Retracted** — it does not.
   The harness logs only on an input EDGE and the schedule has no edge after
   read 600, so a healthy run and a wedged one print the same last line. With
   `FN64_HEARTBEAT` the route runs past read 2,423. See the blocker ledger's
   2026-08-07 retraction. What remains open is the positive claim that the
   entrance presentation and a match are actually reached.
2. **Only WM2000 boots.** The other four AKI titles need a generated crate
   inventory, not speed.
