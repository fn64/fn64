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

### 10. State both ratios, or you have said nothing
There are two different "how fast is it" numbers and they are routinely
conflated:

| | question it answers | target |
|---|---|---|
| **wall ms per emulated VI field** | does a frame fit in the budget? | **16.667 ms** |
| **wall-versus-virtual** | how much slower than the console? | **1.000x** |

They diverge whenever the guest does not emit fields at its nominal rate.
The "2.4x slower than hardware" figure that circulated in this session is
40.88 ms per VI field against a 16.67 ms budget — but wall-versus-virtual over
the same span is only **1.096x**, because WM2000 *during boot* produces fields
at ~27 Hz, not 60 Hz. Both numbers are correct; each is wrong as an answer to
the other's question, and quoting one alone has misled twice.

**The gap is a property of the span, not a standing discount.** Measured over
sustained rendering rather than boot, the guest emits at **59.6 Hz** —
essentially nominal — and the two ratios collapse onto each other: 2.07x the
frame budget and 2.057x wall-versus-virtual. So the reassuring reading of the
first divergence ("we're really only 1.1x off") does not survive contact with
a rendering route. Report the field rate alongside both ratios; it is the
quantity that says whether they should agree.

`FN64_FRAME_CENSUS=1` prints both on adjacent lines, plus the guest's actual
field rate. It is not possible to report one without the other.

### 11. A frame-time distribution needs frames in it
The standard route (`FN64_BLOCK_MAX_STEPS=19523`) has `gfx_submits=0`. It
renders nothing, so every latency statistic over it describes an idle guest.
The census prints span graphics submits next to the distribution and warns
outright at zero, because a beautiful p99 over a route that drew nothing is
the most plausible-looking wrong number available here.

### 12. A large byte count is not a bottleneck
Bytes moved and time spent are different quantities, and reasoning from the
first to the second is not measurement — it is the plausible-sounding story
that rule 1 exists to stop.

Earned on 2026-08-07. `vi::scanout` cloned a `Framebuffer` whose derived
`Clone` copied two depth buffers — `depth` and `encoded_depth` — that nothing
in the scanout chain reads: 600 KiB of every 975 KiB clone, **5.92 GB over
9,637 fields**. The size of that number was treated as sufficient reason to
dispatch. Eliminating all of it moved `vi_present` from 34,848.9 ms to
35,142.9 ms: **+0.84%, the wrong direction, inside noise.** Mean field time
34.85 → 34.94.

The copy was never the cost. VI's 11.6% is per-pixel filter work —
`filter_scanout` gathers eight neighbours across three channels for every
full-coverage pixel, and `restore_rgba16_component_bounded_v1` alone is 2.84%
self time. A bulk `memcpy` runs at streaming bandwidth; a gather-and-blend
over the same bytes does not, and only the profile distinguishes them.

The change was kept, labelled as measuring zero, because it deletes provably
dead work and carries two invariant tests — one pinning the *premise* (output
pixels must not depend on source depth) rather than the optimization, so a
future depth-reading filter fails loudly instead of silently scanning out
against `INFINITY`. Keep it for correctness; do not let it be remembered as a
perf win.

## Measuring the 60fps bar

**`reference/wm2000-routes/render-benchmark.zsh`** — one command, produces a
frame-latency distribution over sustained rendering. Before it existed
(2026-08-07) the "guaranteed 60fps" bar had no test at all.

It runs `entrance-to-match.schedule` for 1.5M steps in the **headless** lane
and reports p50/p95/p99/max, the count of fields over 16.667 ms, and both
ratios above. Headless deliberately: it isolates **guest + runtime** cost,
which is the open question, and it needs no display server. The windowed shell
reports the same percentiles per 60-frame heartbeat (`2676139`) and
additionally pays blit and present — the two lanes are complementary, and a
headless number must never be quoted as a player-experienced frame time.

**The steady-state window.** Boot and first render are a transient: the first
fields take hundreds of milliseconds while overlays activate and shards fault
in, and one such sample owns `max` for the rest of the run. The census gates on
graphics submits (`FN64_FRAME_CENSUS_WARMUP_GFX`) rather than a step or field
count, because submits are the direct evidence that the guest is rendering and
the transient ends exactly when they begin climbing. Gated fields are still
counted and reported separately; nothing is dropped silently.

The default boundary is `300`, picked from the submit trajectory rather than
guessed. Per 50,000 scheduling steps the guest submits:

| steps | 0-50k | 50-100k | 100-150k | 150-200k | 200-250k | 250k+ |
|---|---:|---:|---:|---:|---:|---:|
| new submits | 0 | 106 | 175 | 171 | 261 | ~175 each |

Nothing renders for the first 50k steps; the rate then reaches its steady
~175/50k and stays there for the rest of the run. Submit 300 falls just past
that knee, inside the stable regime.

### The measurement, 2026-08-07 at `19d1ab7`

Quiet machine (load 2.6, no concurrent rustc), headless lane, warmup at
submit 300. **8,984 steady-state fields, 4,600 graphics submits across them.**

| statistic | value |
|---|---:|
| p50 | **43.31 ms** |
| p95 | 60.81 ms |
| p99 | **76.64 ms** |
| max | 132.95 ms |
| mean | 37.60 ms |
| fields over 16.667 ms | **8,123 of 8,984 — 90.4%** |
| `holds_60fps()` | **false** |

**RATIO A — 37.60 wall ms per emulated VI field against a 16.667 ms budget:
2.26x.**
**RATIO B — 2.254x wall-versus-virtual**, with the guest emitting at
**59.9 Hz**, i.e. nominal.

So the honest statement of the goal is: **sustained rendering costs 37.6 ms per
frame and 60fps needs 16.667 — we are 2.26x away, and must remove 20.9 ms per
frame.** The p99 is 76.6 ms, so a worst-case bound needs 4.6x. That converts
"guaranteed 60fps" from an unfalsifiable goal into a target with a number.

Two things this measurement settles:

1. **The two ratios agree here (2.26x vs 2.254x)** because the guest is at
   59.9 Hz. The earlier 40.88 ms / 1.096x pair was a *boot-phase* artifact of a
   27 Hz guest. There is no discount available on a rendering route.
2. **The tail is not dominated by outliers.** 90.4% of fields miss the budget
   and p50 alone already misses it by 2.6x. This is a throughput problem across
   the whole distribution, not a few spikes — so spike-hunting is not the lever;
   the 37.6 ms mean is.

The excluded transient was 595 fields over 21,973 ms (mean 36.9 ms) and
contained a 1,475 ms field; including it moved p50 from 43.31 to 25.26 ms and
`max` from 132.95 to 1,475 ms, which is exactly the startup-in-the-p99
distortion the window exists to prevent.

**Multi-field advances are normalized, and this is load-bearing.**
`GuestDrain::before_step` cannot advance virtual time while the guest has
runnable work, so a menu transition that stays runnable for 19,112 steps
reaches no field boundary until it quiesces — then one `advance_virtual_time`
commits all 22 overdue fields at once. Charging that span to a single frame
reports a three-second frame nobody experienced (`8690d36` attributed exactly
this by counter: the guest was running *faster* than average throughout). The
census divides each advance by the fields it committed, counts all of them
against the budget, and separately reports the worst raw advance with its field
count — so a genuine one-field stall stays visible instead of being averaged
away.

The gate is `FN64_FRAME_CENSUS=1`, implemented in
`crates/fn64-abi/src/frame_census.rs` and hooked into `advance_virtual_time` —
the one seam both lanes cross. It is in `fn64-abi` on purpose: that crate is
neither hashed into the program identity nor subject to rule 8's 32-crate
rebuild, and `examples/wm2000-block-boot/src/main.rs` is both (`build.rs` reads
it into `DISPATCH_SOURCE_SHA256`), so putting a diagnostic there would move the
canonical digest.

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
- **`with_executor`'s `RefCell` borrow.** One call per scheduling step from
  `run_one_step`; its 11% self time is the coroutine resume *inside* its closure
  (71.59% inclusive), not the borrow. Rule 2 in a new costume.
- **`RdramView::read_u8`, treated as a guest-load cost.** 92.7% of its samples
  come from the mutation journal (`read_snapshot`) and bootstrap validation, not
  from guest loads. It is guard work filed under a structural-looking name.
- **The device fabric.** Already zero: `FN64_DEVICE_ADVANCE_CENSUS` reports
  `no samples` on the deep route, because `advance_clock_if_idle` takes every
  call.

See `structural-half-is-mostly-guard.md` — the caller attribution behind all
three, and why a p99 frame-time bound cannot be measured on **this** route at
all (`gfx_submits=0`; it renders nothing). That is a fact about the 19,523-step
route, not about the runtime: the 1.5M-step render route does sustain rendering
and `render-benchmark.zsh` measures a p99 over it. See "Measuring the 60fps
bar" above.

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

## The goal is 1.0x, not "good enough"

Stated by the project owner 2026-08-07: **WM2000 fully playable through the
fn64 recomp and runtime means faithful runtime performance.** 2.4x hardware is
~25 fps against a 60 fps target and does not qualify. Earlier framing in this
session that treated 2.4x as possibly sufficient was wrong and is retracted.

**Now measured rather than extrapolated** (see "Measuring the 60fps bar"):
sustained rendering runs at **37.60 ms/field, 2.26x the budget, 26.6 fps**,
with 90.4% of fields over 16.667 ms. The estimate above was close, but it was
an estimate from a route that rendered nothing; this is 8,984 rendered fields.
**The gap to close is 20.9 ms per frame.**

### What full speed requires, from the measured split

| | share | verdict |
|---|---|---|
| mutation journal / digests | 34% | removable (guard) |
| `changed_ranges_from_view` | 8% | removable (guard) |
| mprotect syscalls | 5% | removable (guard) |
| ~~device fabric — PI/SI/VI/AI~~ | ~~12.5%~~ | **struck — measures zero** |
| ~~`RdramView::read_u8`~~ | ~~11%~~ | **guard, not structural — 92.7% journal** |
| ~~`with_executor` dispatch~~ | ~~11%~~ | **the resume it wraps, not dispatch** |
| per-instruction translation | ~4% | structural (was billed 11%) |
| recompiled guest code | 2.86% | runs at 0.09x, ~11x faster than console |

**The four struck/corrected rows were re-measured 2026-08-07 with caller
attribution** (`scripts/wm2000_callers.py`) and a census, not self time alone.
Three of the four "structural" rows were misclassified; `read_u8` in particular
double-counts the journal already charged at 34%. There is no separate 50%
structural half to attack. See `structural-half-is-mostly-guard.md`.

**Removing the entire guard lands near 1.27x — 47 fps, still not 60.** So
"a release build without the correctness apparatus runs at hardware speed" is
false, and that inference should not be drawn from the 2.86% figure. Half the
remaining cost is *being an N64*: emulating its peripherals, its memory, and its
scheduler.

Reaching 1.0x therefore needs **both** halves:
1. the guard made cheap enough to leave on, or cleanly optional
2. genuine work on the structural half, which **nobody has attacked yet** —
   every optimization this session targeted the guard

Note `FN64_FAST_MUTATION_JOURNAL=1` already measures **zero** difference
(435 ms vs 441 ms): the barrier absorbed that cost, so the removable 47% is not
sitting idle waiting to be switched off.
