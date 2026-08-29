# WM2000 30 Hz optimization loop

Status: active implementation and validation plan, updated 2026-08-24.

## Sustained-play blocker at the catch-up merge

The 2026-08-24 clean `main` Metal/CoreAudio run is **not yet certified
playable**. Its fresh whole-ROM emit and shell build reached swap 240 and
reproduced raw-DPC member ordinal 4091 exactly. The run also confirmed that all
450 AI buffers before the trap were zero-valued. Short 180-pump windows had
crossed a nominal 16.667 ms host-field p95, but that horizon ended near swap 120
and did not exercise either sustained failure.

The failure mechanism is localized and the candidate fix is implemented.
Planning now installs one move-only pending-task value atomically, binding each
member's carry-in state to only a coarse `ComputeCandidate` hint. Exact
execution-time program, TMEM/tile, draw-state, and batch-order admission occurs
before color-generation reservation and produces either a sealed
`ComputeEligible` deferred completion or a named CPU disposition. Exact
admission rejection and an ordinary, non-deferred completion are the only
states routed to the ordered CPU path; genuine validation and corruption errors
remain loud. Focused content-free tests prove mid-batch planning failure
installs no partial state, both non-admitted and non-deferred triangles retain
their ordinary CPU completion, and a live Metal `compute -> CPU -> compute`
sequence publishes all three generations in order. The first sustained
candidate run passed the former ordinal-4091 refusal but exposed the missing
completion-shape check at the same swap-240 frontier. After adding that typed
check, ten consecutive clean 800-pump Metal/CoreAudio runs completed without a
trap and retained identical heartbeat hashes through swap 360. The deterministic
ordinal-4091 blocker is therefore closed; this is not yet a playability claim.

Audio and timing claims are blocked by the same sustained-run frontier. In the
longer run, the first 450 audio buffers remained zero-valued while underrun
slots increased. Warm host-pump p95 ranged from 16.76 to 18.63 ms, with
5.8--13.3% of pumps over 16.667 ms. WM2000 itself still presents distinct game
frames at 30 Hz; the 60 Hz target is host-field work headroom, not 60 distinct
game frames. The reliability bar for extension headroom is p95 at most 14 ms,
p99 at most 15.5 ms, maximum at most 16.667 ms across ten sustained runs, stable
swap hashes, and no audio-underrun or late-callback growth.

Those ten runs expose the next repeatable frontier: drawn-frame p95 was
38.927--39.397 ms, 14.0--14.8% of all 800 measured pumps missed 16.667 ms, and
the swap-360 window missed on 59/120 pumps. Audio became nonzero by swap 300 in
every run, but its ring emptied and underrun slots grew sharply during the heavy
phase; late callbacks remained zero. Fresh profiling can now target this
reachable route. The first attribution pass will separate GPU host preparation,
dispatch span, wait, map/readback, and CPU fallback-member categories before
selecting the next optimization.

## Goal and acceptance bar

The target is the all-Rust `rs + wgpu` play lane at WM2000's native 30 Hz.
One drawn frame is measured directly between consecutive committed VI swaps;
it is not inferred from a pump population or from the game's nominal cadence.

A candidate reaches the performance bar only when all of the following hold:

1. the post-warmup swap-gap histogram is at least 97% gap two;
2. drawn-frame p95 is at most 25 ms and p99 is at most 28 ms, reserving
   roughly 5--8 ms for later rendering extensions rather than landing on the
   deadline;
3. no drawn frame exceeds the hard 33.333 ms budget in any certification
   window;
4. the result repeats for ten consecutive clean runs on a quiet machine;
5. the required framebuffer/differential gates remain unchanged.

The percentile headroom and zero-over-budget requirements intentionally make
“reliable 30 Hz” stricter than a mean below 33.333 ms. A barely compliant
renderer leaves no room for extensions and does not satisfy this goal. Until
all five conditions hold, the status is “not verified.”

`tools/summarize_wm2000_pump_census.py` consumes the existing pump-census
sequence and emits a path-free JSON receipt containing the exact swap-gap and
swap-to-swap latency distributions. Set `FN64_PUMP_CENSUS_SEQUENCE` equal to
`FN64_PUMP_CENSUS_PUMPS`; otherwise the summarizer refuses the incomplete
window. The parser accepts both the legacy 15-field sequence rows and the
expanded 28-field rows, and 40-field rows carrying the existing ABI session
and task-batch clocks. Expanded rows add renderer completion ordinals and task
phase deltas; the v5 receipt folds those deltas between consecutive swaps
under `task_cpu_phase_frames` and `abi_task_phase_frames`. Legacy input remains
valid and reports optional sections as unavailable rather than turning absent
counters into zero-cost claims.

An Observe/Suppress presentation-cache gate also requires exactly 1,600
contiguous `[present-dependency-seq]` rows per lane. Compare the original logs
with `tools/compare_wm2000_present_dependencies.py`: it requires equal
per-pump Cacheable identities or typed Uncacheable reasons while leaving
exact-hit and redraw/suppress disposition lane-specific. This makes a final-
pump omission or changed dependency population a failed A/B, not an inferred
cache win. Canonical identity includes overscan/zoom policy and a SHA-256 over
the geometry, blanking state, and framebuffer bytes. The FNV heartbeat,
generation/invalidation counters, and separately timed probe cost are
diagnostics and are deliberately excluded from cross-lane identity.

For the ordinary path, build the linked shell once and run:

```sh
scripts/benchmark-wm2000-render.zsh --rom /path/to/wm2000.z64 \
  --label baseline --runs 2
```

The runner defaults to warmup 300 / measurement 800, refuses to start while a
Cargo or rustc process is active, forces the `wgpu` renderer, bounds each run,
and writes mode-0700 logs plus JSON receipts outside the repository. It does
not rebuild. `--phase-profile` arms the heavier phase counters for attribution;
ordinary before/after timing leaves them explicitly off. Environment switches
for a candidate are inherited, so the same binary can be run in `A/B, B/A`
order without changing this runner.

For presentation-cache certification, the paired runner alternates order and
invokes the canonical comparator after every pair:

```sh
scripts/benchmark-wm2000-present-cache.zsh --rom /path/to/wm2000.z64 \
  --bin /path/to/the/exact/fn64 --output-dir /private/tmp/present-cache \
  --pairs 10
```

It requires exactly 1,600 measured pumps per lane and stops at the first
missing receipt or dependency mismatch. Ten pairs provide the required 20
clean scheduling runs; the per-run timing receipts remain separate rather
than being replaced by an aggregate-only pass.

`FN64_RSP_DPC_TASK_CENSUS=1` reports the physical DPC runs captured by each
completed RSP task, including the original END-write count, coalesced run
sizes, incomplete-command stalls, and the run containing each FullSync. This
distinguishes unavoidable DMEM-ring wrap boundaries from renderer transaction
boundaries before any batching change is attempted. The disabled path is one
cached boolean check per RSP task; command scanning and allocation occur only
while armed.

## One-variable loop

Every optimization is handled as one transaction:

1. **Baseline.** Build once, verify the resolved renderer is `wgpu`, check the
   machine is quiet, warm up for 300 pumps, then retain an 800-pump sequence.
2. **Profile.** Capture a Time Profiler trace from that exact build and window.
   Export only its `time-profile` table; do not use `sample`, shared kdebug
   stack fragments, or inferred ASLR slides. Summarize it with
   `tools/summarize_xctrace_time_profile.py` so exclusive rows and selected
   main-image callers are repeatable and path-free.
3. **Choose one hotspot.** State its measured exclusive cost and a falsifiable
   mechanism. Prefer work repeated per pixel or per scanline over command-level
   checks unless the profile says otherwise.
4. **Add a measurement control.** When practical, keep old and candidate paths
   selectable in the same binary. This permits counterbalanced `A/B, B/A`
   pairs without rebuild or scene drift.
5. **Implement only that candidate.** Do not combine unrelated changes in one
   timing result.
6. **Correctness gate.** Run focused byte-identity tests and a mutation test
   that proves the gate fails when the mechanism is removed. Preserve loud
   boundary errors; diagnostic work may be omitted only when it cannot affect
   guest-visible state.
7. **Counterbalanced timing.** Run at least two `A/B, B/A` pairs with identical
   warmup and horizon. Compare swap-to-swap drawn-frame distributions and the
   relevant phase total. A single sequential before/after is not evidence.
8. **Decision.** Keep the candidate only if both pairs agree in direction and
   the effect exceeds noise without worsening p95/cadence/correctness. Revert a
   null or negative candidate immediately and record that result.
9. **Re-profile.** Confirm that the named hotspot fell and that cost did not
   merely move to another frame. The new profile selects the next candidate.
10. **Commit the evidence unit.** Code, tests, behavioral docs, timings, run
    counts, and the measurement control belong in the same commit.

This loop repeats until the acceptance bar holds. Only then run the repository's
full differential suites and ten-run certification series.

## Current ordered queue

1. Move intermediate color-target ownership between scheduled commands instead
   of cloning and immediately dropping the same full-target bytes. **Measured
   and retained; evidence below.**
2. Remove the next measured copy at the command executor boundary by passing
   the owned accumulator into raw-triangle/texrect execution. **Measured and
   retained.**
3. Re-profile and use full call paths, not leaf names alone. The shared
   texrect blend/combiner helpers are now attributed mostly to raw triangles;
   optimize their triangle callers before the minority texrect callers.
4. Continue reducing fixed VI cost for extension headroom, but do not mistake
   it for the variable RDP tail. Typed five-bit restoration is retained below.
5. Reconsider command-level validation only when the fresh profile attributes a
   material cost to it. Move stable invariants into types or once-per-command
   validation; never delete guest-visible RDP semantics to meet the budget.

The queue is deliberately provisional after item 2: each fresh profile and
drawn-frame population split, not this document, chooses the next optimization.

## First combined result

One release binary was measured in counterbalanced candidate/control order at
warmup 300 / measurement 800. The candidate omits diagnostic GPU TMEM
projections and enables parallel VI restoration; the same-binary control sets
`FN64_DIAGNOSTIC_TMEM_PROJECTION=1` and `FN64_PARALLEL_VI_DITHER=0`.

| Order | Mean ms/drawn | p95 ms/drawn | Gap two |
| --- | ---: | ---: | ---: |
| candidate 1 | 33.304 | 51.354 | 100% |
| control 1 | 39.223 | 59.244 | 100% |
| candidate 2 | 33.097 | 51.033 | 100% |
| control 2 | 38.958 | 59.487 | 100% |

Both pair orders agree: the combined candidate removes about 5.9 ms from the
mean and 8.2 ms from p95. It remains outside the reliability bar because its
p95 is about 51.2 ms and 57.4–58.9% of drawn frames exceed 33.333 ms.

A fresh Time Profiler capture of the candidate recorded 27,437 one-millisecond
samples. The largest named exclusive CPU rows are parallel VI restoration
(3,743 samples across Rayon workers), `_platform_memmove` (3,171), texrect
blend/write (1,522), SHA-256 compression (1,050), combiner cycle evaluation
(744), texrect combine (714), VI bilinear resampling (642), and the two scalar
raw-triangle specializations (555 and 347). Resolving `_platform_memmove` by
its immediate caller attributes 1,646 samples to `stage_color_commands` and
472 to `execute_scheduled_raw_triangle`. The next candidate therefore targets
the redundant full-target ownership copy in `stage_color_commands`; the profile
does not justify deleting command-boundary correctness checks.

## Accumulator ownership result

The next release binary moves each intermediate completion's owned device-byte
buffer into the schedule accumulator and retains only the final completion.
`FN64_MOVE_COLOR_ACCUMULATOR=0` preserves the former clone in the same binary.
The adapterless three-fill composition fixture observes pixels owned by all
three commands and passed 10/10 consecutive runs on both candidate and control.

| Order | Mean ms/drawn | p95 ms/drawn | Gap two | Over 33.333 ms |
| --- | ---: | ---: | ---: | ---: |
| candidate 1 | 30.729 | 46.337 | 100% | 50.1% |
| control 1 | 33.657 | 54.840 | 100% | 58.6% |
| control 2 | 33.839 | 53.864 | 100% | 59.1% |
| candidate 2 | 30.825 | 46.875 | 100% | 51.9% |

Both pair orders agree: the ownership move removes 2.93--3.01 ms from mean
drawn-frame time and 6.99--8.50 ms from p95. A fresh 26,676-sample Time
Profiler capture attributes 432 `_platform_memmove` samples to
`stage_color_commands`, down from 1,646 (74% lower), while total memmove falls
from 3,171 to 2,016 samples. The named cost fell rather than moving elsewhere.

The phase-armed drawn-frame receipt then separates the remaining mechanism.
Over-budget frames average 2.99 graphics tasks and 30.85 ms of RDP work;
within-budget frames average 1.59 graphics tasks and 15.47 ms of RDP work.
Their 19.04 ms wall-time delta contains a 15.38 ms RDP delta, while VI
presentation is 0.09 ms lower in the over-budget population. The tail is
variable RDP workload, not VI presentation or command-boundary checks. The
post-change profile's next full-target copy is 531 memmove samples attributed
to `execute_scheduled_raw_triangle`, which selects queue item 2.

## Post-ownership attribution and typed VI restoration

A fresh 1,200-pump Time Profiler capture after the ownership, sealed-TMEM, and
grouped-VI changes recorded 34,827 one-millisecond samples. The largest active
rows were VI dither restoration (3,235), blend/write (2,160), memmove (1,632),
combiner-cycle evaluation (1,340), SHA-256 compression (1,207), raw-triangle
scalar traversal (749 + 599), and texel combining (915). Full call paths changed
the interpretation of the shared helper names: raw triangles account for about
1,295 blend/write samples and 611 texel-combine samples, while direct texrect
callers account for about 330 and 207 respectively.

Three same-binary candidates were rejected and removed: preparing one-cycle
texrect combiner selectors, preparing the common texrect blend, and extending
scanline parallelism to depth-bearing triangles. None improved both mean and
tail latency. The existing depth-free parallel raster path remains valuable:
its ABBA control measured about 1.0 ms lower mean and 3.0 ms lower p95 than the
scalar lane.

VI restoration previously rechecked per pixel that components produced by
`byte >> 3` were five-bit and that a local neighborhood held at most eight
entries. `Rgba16Rgb5` now carries the component invariant in its private type,
and a fixed eight-entry array bounds neighborhood storage while retaining a
loud slice-bound trap. The checked API remains selectable with
`FN64_TYPED_VI_DITHER=0` as the same-binary control.

| Order | Mean ms/drawn | p95 ms/drawn | p99 ms/drawn | Over 33.333 ms |
| --- | ---: | ---: | ---: | ---: |
| control 1 | 24.431 | 39.290 | 42.882 | 25.4% |
| candidate 1 | 24.326 | 39.136 | 42.724 | 25.1% |
| candidate 2 | 24.268 | 39.152 | 42.706 | 25.4% |
| control 2 | 24.634 | 39.593 | 42.963 | 26.4% |

Both pair orders agree. A second 1,200-pump profile reduced the named VI
restoration row from 3,235 to 2,155 samples (33%); total sampled time fell from
34,827 to 33,510. The candidate and checked paths produced identical hashes for
120 live frames, and the shared-filter plus 47-test scanout set passed 10/10
consecutive runs. This is retained as fixed-cost headroom, but the acceptance
bar remains unmet: p95 is still about 39.1 ms and roughly one quarter of drawn
frames miss 33.333 ms.

## Prepared two-cycle triangle combiner

The post-VI profile changed the next combiner experiment's scope. Earlier
one-cycle preparation was limited to texrect execution and produced no gain,
but full call paths attributed most combiner work to raw triangles. WM2000's
measured raw-triangle programs are predominantly two-cycle, so the retained
path decodes both cycles' sixteen selectors once per draw and passes the typed
prepared program through both scalar and parallel scanline traversal.
`FN64_PREPARED_TRIANGLE_COMBINER=0` retains the original per-pixel decode as a
same-binary control; absent selects the prepared path, and other values trap.

| Order | Mean ms/drawn | p95 ms/drawn | p99 ms/drawn | Over 33.333 ms |
| --- | ---: | ---: | ---: | ---: |
| control 1 | 24.381 | 39.278 | 42.907 | 24.7% |
| candidate 1 | 24.277 | 38.784 | 42.688 | 25.1% |
| candidate 2 | 24.352 | 39.110 | 42.715 | 25.1% |
| control 2 | 24.474 | 39.535 | 43.082 | 26.1% |

Both pair orders improve mean by 0.10--0.12 ms and p95 by 0.43--0.49 ms.
A fresh 1,200-pump, 34,465-sample Time Profiler capture reduced the combiner
cycle evaluator from 1,340 samples before this change to 1,091 (19%); selector
construction itself accounted for one sample, confirming that work moved to
the draw boundary rather than another hot leaf. The prepared and checked paths
were byte-identical for 120 live frames. Exact equivalence over the eight
measured WM2000 programs plus the focused raw-triangle suite passed 10/10
consecutive runs.

This is retained as additive fixed-cost headroom, not as closure. Current p95
is still about 39 ms, leaving roughly 6 ms to reach 30 Hz reliability and about
14 ms to reach the 25 ms extension-headroom target. The same capture's largest
active renderer leaf is now `blend_and_write_pixel` (2,396 samples), followed
by VI restoration (2,323), full-target copies (1,585), SHA-256 (1,212), and
combiner arithmetic (1,091). Because over-budget frames carry roughly twice
the RDP work of within-budget frames, the next loop must keep prioritizing
per-fragment triangle costs and copies over fixed presentation work.

## Remaining budget and post-combiner disproofs

The best retained p95 is 38.8--39.1 ms. Reliable 30 Hz therefore still needs
about 5.8 ms from the tail, while the 25 ms extension-headroom bar needs about
14 ms. Mean is already 24.3 ms, so optimizing only average or off-critical-path
CPU work cannot satisfy either tail requirement.

The post-combiner trace accounts for the next active renderer costs as follows
(exclusive one-millisecond samples over the same 1,200-pump capture):

| Mechanism | Samples | Critical-path interpretation |
| --- | ---: | --- |
| blend, coverage, and target write | 2,396 | At least 1,382 samples are directly under raw-triangle traversal; the largest per-fragment target. |
| VI restore + resample | 3,195 | Fixed presentation cost; useful headroom, but the population split disproves it as the variable tail. |
| full-target and capture copies | 1,586 | Distributed across capture (279), command staging (240), fill (185), raw-DPC execution (174), and smaller owners; no single remaining clone explains the tail. |
| SHA-256 | 1,212 | 398 samples are sealed-TMEM revalidation, 229 are commit-side effect verification, and 162 are color-command effects; not all are on the swap critical path. |
| combiner arithmetic | 1,091 | Still per fragment after selector preparation; arithmetic, not decode, now dominates. |
| scalar raw-triangle bodies | 1,487 | Per-fragment traversal outside named texture/combiner/blend leaves. |
| TMEM address + decode + sample | 1,258 | Per textured fragment and therefore correlated with heavy RDP frames. |

Two additional same-binary candidates were rejected and removed:

- Skipping RGBA16 destination-color expansion when `IM_RD` is clear was flat
  in one pair and worse in the reverse pair (candidate versus control:
  mean -0.009/+0.139 ms, p95 -0.173/+0.235 ms).
- Omitting the sealed proposal's second internal hash at GPU binding removed a
  398-sample aggregate leaf but worsened both critical-path pairs (mean
  +0.196/+0.127 ms, p95 +0.266/+0.115 ms). This is direct evidence that the
  aggregate hash row is not a useful proxy for swap-tail latency.

The remaining work list is consequently narrower:

1. Add a low-overhead per-draw census keyed by resolved combiner, blender,
   texture mode, pixel count, and elapsed fragment time. Use it to identify a
   state-keyed specialization with a predicted multi-millisecond tail effect;
   do not specialize on game addresses or content.
2. Feed runtime captures kept outside git into a bounded raw-DPC packet replay
   benchmark, so fragment candidates can be measured without a full boot and
   2.5-minute linked-shell LTO cycle. The replay must compare the candidate's
   completed bytes and effects to the general path on every iteration.
3. Fuse the hottest resolved texture/combiner/blender program into one prepared
   fragment pipeline and evaluate multiple adjacent pixels per loop. Exact
   equivalence to the general functions remains the oracle; all admission and
   boundary traps stay at draw construction.
4. If that CPU specialization cannot remove at least 3 ms from p95, stop
   micro-optimizing. The 14 ms headroom gap then requires an exact batched or
   compute raster path using integer RGBA16/TMEM semantics and one bounded
   readback, rather than further trimming checks from scalar fragments.
5. Continue ownership work on the individually attributed capture/staging/fill
   copies only where a move-only payload can eliminate both the copy and its
   duplicate digest verification. Preserve the external effect comparison;
   the rejected TMEM candidate shows that deleting internal work without a
   critical-path result is not progress.

## Draw census and architecture pivot

A 600-pump diagnostic run with `FN64_DRAW_CENSUS=1` timed only each raw
triangle's raster call and grouped the first 75,000 draws by the complete
resolved combine/other-mode program, cycle count, target format, texture and
perspective flags, and depth wiring. All eight observed keys were textured,
perspective RGBA16 draws without depth. The last complete census prefix spent
2,117.667 ms in raster calls; the five leading exact keys accounted for
2,083.674 ms (98.4%). The distribution is broad enough to rule out the earlier
two-cycle fog state as a closure candidate:

| Rank | Combine low/high | Evaluation | Draws | Pixels | Raster ms | Share |
| ---: | --- | ---: | ---: | ---: | ---: | ---: |
| 1 | `fc5196a3/112cfe7f` | 1 | 14,180 | 30,093,140 | 1,021.666 | 48.2% |
| 2 | `fc309661/552eff7f` | 1 | 5,724 | 14,812,277 | 484.401 | 22.9% |
| 3 | `fc1596a3/f0fffe38` | 2 | 2,060 | 6,689,362 | 213.916 | 10.1% |
| 4 | `fc15fea3/f00ff23f` | 2 | 49,536 | 4,153,325 | 209.099 | 9.9% |
| 5 | `fc1596a3/f0fffe38` | 2 | 2,322 | 4,169,956 | 154.592 | 7.3% |

A fresh 1,200-pump phase profile independently keeps the variable tail on the
RDP path. Slow pumps averaged 23.147 ms of RDP work versus 1.631 ms for fast
pumps, accounting for 83.4% of slow-population excess. Across drawn frames,
over-budget frames averaged 26.862 ms of RDP versus 17.135 ms within budget, a
9.727 ms difference; presentation differed by only 0.023 ms. The same build's
Time Profiler trace retains the scalar fragment stack as its largest active
renderer cluster: blend/write 2,339 samples, combiner-cycle arithmetic 1,080,
the two raw-triangle scalar bodies 1,577, and TMEM address/decode/sample 1,403.

Two CPU follow-ups were rejected:

- Incrementally stepping S/T/W over a proven adjacent-pixel run measured
  559.607 ns per covered pixel in the headless release raster replay versus
  547.025 ns for exact per-pixel plane evaluation. The extra state and branch
  cost more than the avoided wide arithmetic, so the candidate was removed.
- Lowering the parallel-scanline cutoff from 4,096 to 1,024 pixels was worse
  in both orders of a same-binary `A/B, B/A` run. Candidate versus control was
  27.184/26.266 and 26.772/26.120 ms mean, and 40.156/39.359 and
  39.955/38.868 ms p95. Rayon scheduling smaller draws adds 0.65--0.92 ms mean
  and 0.80--1.09 ms p95, so the retained threshold remains 4,096.
- Caching the first covered subsample between the row's at-most-sixteen edge
  interval endpoints measured 580.956 ns per covered pixel versus 577.803 for
  the direct row scan. Interior pixels normally pass its first sample test, so
  endpoint bookkeeping is an added cost; the candidate was removed.

These results trigger item 4's stop condition. The CPU raster specialization
budget is smaller than the roughly 5.8 ms reliability gap and far smaller than
the roughly 14 ms extension-headroom gap. The next implementation unit is an
exact integer compute raster path for the census-defined textured RGBA16
subset, with CPU raster as the byte-for-byte oracle and fallback. It must batch
ordered draws against one device-resident target, preserve command-order
barriers where target reads make them observable, and perform one bounded
readback at the existing guest-effect publication boundary. Program admission
is keyed only by typed RDP state; unsupported state remains on the CPU path.
The [compute-raster execution plan](WM2000-COMPUTE-RASTER.md) defines the
ordered implementation and kill gates.

## Production execute closure and the packet-boundary frontier

A fresh linked `rs + wgpu` build at `72293b35` adds
`FN64_RAW_DPC_EXEC_CENSUS=1`, a diagnostic-only nested timer at raw-DPC
submission boundaries. Its disabled path performs no clock reads or atomic
updates. The armed path reports every 10,000 execution views and distinguishes
the plan view, staging, color staging, coordinator completion, and the retained
GPU-draw validation seam. Color staging is further divided by command kind and
final effect construction. Nested children are printed as residuals rather
than added to their parent twice.

The first 800-pump run measured 399 drawn frames at 27.261 ms mean, 39.959 ms
p95, and 31.1% over 33.333 ms. Its session census observed 30,745 submissions:
plan was 1,355.0 ms, execute 6,442.4 ms, commit 117.3 ms, and finalize 15.6 ms.
At the comparable 30,000-submission prefix, the execute census accounted for
6,279.1 ms. Of that, 4,835.0 ms was color staging and 1,264.5 ms was non-color
staging; view residual, coordinator completion, and diagnostic draw validation
together were only 179.6 ms. The independent timers therefore close closely
enough to select a mechanism, and reject completion bookkeeping or diagnostic
validation as the remaining wall.

A second 800-pump run split the 4,897.5 ms color-staging prefix:

| Color-stage mechanism | Time | Share |
| --- | ---: | ---: |
| raw triangles (161,660 calls) | 3,263.1 ms | 66.6% |
| texrects (18,645 calls) | 896.8 ms | 18.3% |
| final digests and target admission | 291.5 ms | 6.0% |
| fills (32,280 calls) | 288.5 ms | 5.9% |
| schedule, target seed, and loop residual | 157.5 ms | 3.2% |

The finer timers raised this diagnostic run to 27.523 ms mean and 40.551 ms
p95, so its latency distribution is attribution evidence, not an ordinary
performance verdict. A combined draw/execute census then measured 2,219.3 ms
inside the raster call itself for the first 75,000 successful triangles over
60,859,295 declared pixels. The five established state keys still account for
nearly all of that work. This corroborates the command-kind split: raw-triangle
fragment evaluation, rather than its boundary validation, is the dominant
color cost.

The next closure candidate must therefore amortize raster work across a larger
ordered unit. Packet-local compute replacement is already negative because it
uploads, submits, waits, and reads back at each packet boundary. Deleting
effect or transport checks cannot supply the required 5.8 ms reliability gap,
and would weaken the loud-corruption boundary. The remaining architectural
frontier is either an ordered row-parallel batch spanning enough commands to
make small triangles worthwhile, or a device-resident color target whose
typed CPU/DMA/VI observation boundary performs bounded synchronization and
readback. Either candidate must preserve command order, FullSync/fabric
publication, exact guest bytes, and the M9 visibility invariants; performance
alone cannot choose a weaker ownership model.

## Exact finite-RN COP1 single arithmetic

WM2000's linked Rust lane now uses a sealed host-arithmetic fast path for
`ADD.S`, `SUB.S`, and `MUL.S` by default. Admission requires FCSR.RM=RN and
finite-normal-or-zero operands. The result must also be finite normal or an
independently proven exact zero; overflow, underflow, subnormal, NaN, infinity,
and every non-RN operation retain the authoritative `rustc_apfloat` path. A
binary64 witness reconstructs the exact binary32 add/sub result when the
exponent gap is at most 29 and the exact binary32 product for multiplication,
so the helper returns the same Inexact flag consumed by the existing
Cause/Flag/Enable/trap ordering. FCSR writes cache the combined selector/RM
admission in `RecompContext`; ordinary hot operations perform no environment
lookup or diagnostic atomic update.

`FN64_COP1_RN_FINITE_FAST` is the strict control: absence or exactly `1`
enables the route, exactly `0` selects soft float, and every other or
non-Unicode value traps. The optional `cop1-fast-receipt` CPU-runtime feature
records only first-seen fast and fallback operation classes and is absent from
ordinary builds.

The helper passed a boundary-class differential and three million
deterministic operand pairs against `rustc_apfloat`. Its 11-pair release
microbenchmark measured 16,000 ns versus 208,208 ns for 12,288 operations
(13.01x). A complete-instruction-state differential covers stale Cause,
disabled and enabled Inexact, sticky Flags, trapping destination suppression,
destination aliasing, both FS values, non-RN fallback, and exceptional input
and result classes.

The first seven-pair linked PGO screen retained one exact owner identity across
all fourteen processes. In the 66-drawn-frame red/flame interval, paired
residual (`pump - graphics - audio`) saving was 0.604 ms median and the full
drawn-frame wall saving was 0.345 ms median. The follow-up quiet 20-process
counterbalanced stability gate again retained one exact identity. Candidate
full-run p99 was 23.572--24.438 ms in every process, with no candidate frame
above 30 ms; paired median savings were 0.605 ms for the red/flame residual,
0.521 ms for red/flame wall time, and 0.381 ms over all drawn frames. The only
three >30 ms observations were control frames, all raw-DPC dominated rather
than COP1 residual. Finally, ten consecutive fresh candidate processes each
matched all 120 hashes in the current scanout framebuffer tripwire. These
timing results describe the frozen trained linked binary; they do not make an
additive claim with replay-only renderer wins.

## Persistent raw-DPC worker

A 500 Hz Samply profile of the current full intro found 6,166 distinct
`fn64-rdp` host threads during 5,600 pumps. Each task batch created and joined
a new thread even though the backend ownership and one-outstanding-batch
contract already serialized that work. The production wrapper now creates one
persistent worker at backend registration and transfers the backend through
bounded command/completion channels for each batch. Guest execution, ordered
non-RDP writes, publication, presentation, and device state remain on the
emulation thread; a successor batch still cannot overtake its predecessor.

Separate, untrained release-profile full-intro processes provide directional
rather than counterbalanced timing evidence. Across 600 warmup and 5,000
measured pumps, the per-batch-thread control closed 2,466 drawn-frame task
identities with zero mismatches; the persistent-worker candidate closed 2,488
with zero mismatches. Mean drawn-frame pump cost changed from 22.456 to 21.023
ms, mean swap-to-swap interval from 35.790 to 35.292 ms, and mean outside-loop
residual from 0.841 to 0.725 ms. Drawn p95 did not improve (42.449 versus
42.823 ms), so this removes host lifecycle churn but does not claim to close
the red/flame raster tail, visual artifacts, or audio underruns.

## Cache-local prepared texel sampler

The prepared sampler's decoded-texel cache is rebound for every scalar
triangle and for every independently rasterized parallel row. Its direct-map
entries compare the complete addressed texel before returning a hit, so cache
capacity affects only rereads, never the selected texel or snapshot. Reducing
the map from 16x16 to 8x8 cells keeps the short-lived zeroed state cache-local;
an explicit collision test alternates addresses eight cells apart across
Point, Bilinear, and Average filtering and compares every result with the
uncached production sampler.

Fresh release replay binaries measured the exact 140-packet red-transition
window as its original three task batches, with 10 warmups and 100 repeats per
leg in `control, candidate, candidate, control` order. Execute means were
26.071/25.863 ms for the 16x16 control and 25.910/25.563 ms for the 8x8
candidate, paired savings of 0.161/0.300 ms. Total means were 33.502/33.146 ms
control and 33.243/32.829 ms candidate. Every leg retained the same committed
and final RDRAM postimage identities. This is a bounded CPU-cache win, not a
red/flame correctness or tail-closure claim.

## RSP-task batching control

The task-level DPC census observed 169 graphics tasks, 109,255 raw END writes,
and 2,345 address-coalesced runs (13.88 runs per task, maximum 26). Every task
contained exactly one FullSync, always in its final run, and no run ended with
an incomplete command. An RSP task is therefore a real renderer-lifetime
opportunity, but not necessarily a single-target execution unit.

`FN64_RAW_DPC_REPLAY_COMBINE_WINDOW=1` provides a non-certifying control that
concatenates the selected replay window into one synthetic RDRAM command
stream. It does not model the production fabric journal or publication
boundaries. In the captured suffix ending at packet 2,659, four adjacent
submissions executed successfully and produced the same final RDRAM SHA-256 as
the ordinary four-packet replay. Five submissions trapped because a fill named
a range outside the packet's selected color target; six and larger windows
also trapped. This falsifies whole-task concatenation and identifies target
compatibility as a required split condition.

The passing four-submission control did not close meaningful time. In an
`A/B, B/A` native-GPU run with 5 warmups and 50 measured repeats per leg,
ordinary replay measured 5.702/5.694 ms mean total and combined replay measured
5.655/5.650 ms. Execute changed from 2.021/2.013 ms to 1.983/1.979 ms. An exact
compute chain probe resolved both forms to the same three batches and five
draws per repeat; probe time was 1.265 ms ordinary versus 1.285 ms combined.
Transport-call removal is consequently a sub-1% CPU-path optimization in this
control, not the missing 25--33%. Compatibility grouping remains necessary for
device residency, but the next performance variable is lower-level raster
execution and the target upload/readback lifetime it currently imposes.

## Exact RGBA16 fragment terminals with physical coverage

The physical hidden-coverage route initially made the existing specialized
fog combiners fall back through the general coverage, alpha-stage, blender,
pixel-read, and pixel-write functions. That fallback was exact but redundant
for three fully keyed programs. `fc15fea3/f00ff23f` has image read disabled
and stores its coverage-times-alpha result directly under `CVG_DST_CLAMP`.
`fc1596a3/f0fffe38` and the one-cycle `fc309661/552eff7f` both use the same
proved RGBA16 source-over terminal and store `CVG_DST_FULL`. Their admission
matches the complete target format, combiner words, other-mode words, cycle
count, and textured shape; no title address or content selects them.

Direct generic-oracle sweeps cover every primitive and memory coverage count,
all 256 combined-alpha bytes, both coverage-fog dither words, and the complete
one-cycle combiner alpha-pair domain. `FN64_EXACT_FRAGMENT_PROGRAMS=0` is the
strict same-binary control: it retains the earlier specialized fog combiners
with their generic terminal and leaves `fc309661` fully generic. Absence or
exactly `1` enables the closed programs and every other value traps.

The exact red-transition replay ending at captured packet 203499 primes 1,300
earlier packet states and benchmarks 140 packets as their three original task
batches. A same-binary `control, candidate, candidate, control` run used a
plain release build, 10 warmups, and 100 measured repeats per leg. Control
execute means were 22.610/23.039 ms and candidate means were 21.428/21.687 ms,
paired savings of 1.182/1.352 ms. Total means were 30.128/30.683 ms control and
28.978/29.181 ms candidate. Every leg kept committed FNV-1a
`7d0a23e90c2cd54b` and the same final RDRAM postimage identity.
Ten additional fresh candidate processes independently primed the same 1,300
packets and retained both identities in 10/10 runs; their single-sample execute
times ranged from 20.895 to 22.289 ms.
This is replay-only evidence from an instrumentable non-PGO binary, not the
required full-intro visible certification and not evidence that the red/flame
scene itself is correct.

## Fresh full-intro PGO result after exact terminals

The finalized `85ad3fbc` source was rebuilt through a new isolated PGO cycle.
Its instrumented shell trained over 300 warmup plus 6,000 measured pumps with
WGPU and live audio. The merged 18 MiB profile covered 58,952 functions and
902,561 blocks. A separately targeted profile-use build then ran the same fixed
visible population from a fresh process.

The profile-use run produced 2,965 drawn frames from 6,000 pumps. Drawn-frame
mean/p50/p95/p99/max were 13.984/13.872/28.891/32.225/36.853 ms. There were
117 frames over 30 ms and 9 over the 33.333 ms visible-frame budget. All 2,965
task-batch identity closures matched. This supersedes, but is not a marginal
same-binary comparison with, the historical 20.505 ms mean, 34.158 ms p95,
39.358 ms p99, 41.993 ms max, and 199/2,999 budget misses retained before the
timing/render-authority and exact-terminal work.

The renderer result does not close audio or visual correctness. Two heartbeat
windows contained wall intervals of about 364 and 355 ms while their maximum
measured pump work was only about 14 and 16 ms. The audio callback consequently
reported 64,812 non-contention underrun sample slots. That is evidence of a
host-pacing gap outside the measured emulator pump, not permission to attribute
it to RDP execution. No transition-stripe detector or direct visual oracle ran
in this census, and no red/flame, diagonal-striping, or exact A/V cue-sync claim
is made.

## Production texture-filter correction and retained performance frontier

The red/flame investigation first separated the observed artifact from the
earlier transition-stripe bug. The old exact diagonal-stripe detector remained
clean, while current same-frame dumps showed a different pervasive mesh over
the red field and repeated rectangular flame tiles. Disabling raw task
batching, exact fragment terminals, and parallel raster independently in the
same binary did not remove them. A Mupen black-box run showed smoother
textured fades and flames, but its attract sequence was not aligned closely
enough to serve as an exact pixel oracle.

The production capture supplied the decisive state evidence: all 9,132
textured triangles selected Bilinear filtering, and texture rectangles were
780 Bilinear versus 160 Point, while both production CPU raster paths still
always sampled one point texel. Routing Point, three-nearest Bilinear, Average,
and Reserved through one snapshot-bound prepared sampler removed the repeated
rectangular flame tiles in exact current frames 5,200 and 5,500 and softened
the mesh in frame 4,180. Frame 4,200, outside the affected commands, remained
byte-identical. This accounts for the blocky-flame defect; it does not prove
that the remaining red tint, mesh, or every filtered pixel matches hardware.

The first literal multi-read implementation was rejected for performance:
heavy-window p95 pumps rose to roughly 24--31 ms. Preparing TLUT decode and
caching exact addressed texels recovered most of that loss. The retained
release microbenchmark measures Point at 5.846 ns and Bilinear at 6.705 ns per
covered pixel in the same binary (14.7% overhead). A fresh fat-LTO 5,600-pump
live run measured 9.578/6.389/26.091/93.733 ms mean/p50/p95/max, with 913 pumps
over 16.667 ms. Slow-pump RDP work differed from fast pumps by 1.192 ms, but
executor time differed by 8.971 ms and 39.7% of slow-pump wall remained outside
the measured root phases. This is therefore a bounded correctness cost, not a
claim that the performance or audio-underrun frontier is closed. The next
optimization must attack the larger executor/unattributed tail while retaining
the programmed filter semantics and exact output identities.

## Rejected intra-member copyback coalescing

Reverse-order last-write-wins coalescing of overlapping copyback ranges was
tested and removed. On the exact red-transition replay ending at captured
packet 203499, with 1,300 prefix packets, 5 warmups, and 20 repeats, control
and candidate retained the same committed FNV-1a and final RDRAM postimage
identities.
Mean copyback changed only from 1.284 to 1.267 ms and mean total from 34.194
to 34.155 ms. The 0.017 ms copyback reduction is noise-scale and disproves
the estimated 0.4--1.2 ms opportunity for this workload; no production code
or selector was retained.

## Current full-intro PGO certification

The finalized `d67ed98b` source, including programmed texture filtering, the
persistent raw-DPC worker, and the cache-local prepared sampler, was rebuilt
through a new isolated PGO cycle. One instrumented full-intro process trained
over 300 warmup plus 6,000 measured pumps with WGPU and live audio. Fifty
nonempty raw profiles merged into an 18 MiB profile; a separately targeted
profile-use build then ran the same fixed population from a fresh process.

The profile-use run produced 2,965 drawn frames from 6,000 measured pumps.
Drawn-frame mean/p50/p95/p99/max were
15.291/14.626/33.261/35.690/37.730 ms. There were 270 frames over 30 ms and
146 over the 33.333 ms visible-frame budget. All 2,965 task-batch identity
closures matched. Against the older representative visible run retained in
the active brief, this reduces p95 from 34.235 ms, max from 39.865 ms, and the
over-30 count from 416; p99 is effectively unchanged (35.670 versus
35.690 ms). This is not a same-binary marginal comparison.

The earlier `85ad3fbc` PGO census remains faster, but it predates the
production texture-filter correction and therefore does not execute the same
rendering behavior. Its 13.984 ms mean and 28.891 ms p95 cannot be used as a
control for removing the current Bilinear work. The current run is the
authoritative performance baseline for the corrected renderer.

Audio reported 69,724 non-contention underrun sample slots, concentrated in
the heavy later windows. No direct Mupen-aligned pixel oracle, transition-
stripe detector, or cue-sync detector ran in this census. The result therefore
does not claim that audio pacing, red/flame rendering, diagonal striping, or
exact A/V synchronization is fixed.

## Fused ordered sparse-checkpoint materialization

The ordered CPU color path formerly derived every declared guest-write digest
from the final full target, then copied the same slices and derived the same
digests again while sealing each member's sparse publication. The retained
path instead copies each exact slice once and derives both the guest write and
the move-only sparse checkpoint from that payload. The full accumulator still
moves forward as the next ordered member's input. This changes neither journal
order nor the typed generation, coverage, hidden-coverage, or publication
authorities. `FN64_FUSED_SPARSE_CHECKPOINT=0` is the strict same-binary
control; absence or exactly `1` enables fusion, and other values trap.

The surviving exact red-transition capture was replayed through its stable
140-packet, three-task window ending at captured packet 203499, after priming
1,300 earlier packets. Five counterbalanced four-process blocks used 10
warmups and one measured iteration in every fresh process. Across ten control
and ten candidate processes, execute mean/median changed from 24.774/24.793 ms
to 24.141/24.090 ms; total mean/median changed from 32.140/32.164 ms to
31.556/31.442 ms. Per-block execute savings were 0.760, 0.955, 0.035, 0.304,
and 1.111 ms; total savings were 0.745, 1.057, -0.204, 0.205, and 1.115 ms.
Every process retained committed FNV-1a `a3c78e737486ccfa` and final RDRAM
SHA-256 `fe2e2a9a3b1f8415d5a6ffa49611cf00c350772549c1fc63c96c3abfbb770295`; no test
can rederive that external-capture identity from repository content.

A separate 100-repeat phase census attributed 44.129 ms across 330 tasks to
the legacy late sparse-checkpoint pass and 0.357 ms to that late pass with
fusion. Fused materialization moves into color finalization, so this timer
reduction is not itself an end-to-end saving; that instrumented process
measured execute means of 25.171 and 24.351 ms. The sparse-checkpoint suite and
the ordered task-batch publication test each passed ten consecutive clean
runs. This is replay-only evidence from a plain release binary. It does not
claim a visual correction, audio-underrun closure, exact A/V cue sync, or the
required final full-intro PGO result.

The briefed `packet=179/window=180` selector does not end at a FullSync in the
surviving 1,463-packet directory, and a separately isolated middle task did
not retain one committed-byte identity across identical iterations. Neither
invalid population was used as performance evidence.

## Fresh full-intro PGO evidence after sparse-checkpoint fusion

A new isolated PGO cycle trained the native WGPU/live-audio shell for 300
warmup plus 6,000 measured pumps. Fifty raw profiles merged into an 18 MiB
profile containing 59,148 functions and 906,498 blocks. The merged profile's
SHA-256 was `ca7201f99da1525be26d6f3f325724e6b5afdd115f30724be882676399090b24`; no test
can rederive this private transient profile. The separately targeted
profile-use executable's SHA-256 was
`00a53a9c155da347c88270be5e071e88a6d575742ea7a327896fb90581e8835c`; no test
can rederive the removed isolated target artifact.

The fresh visible profile-use process produced 2,963 drawn frames. Drawn-frame
mean/p50/p95/p99/max were
15.113/14.293/32.845/34.705/36.301 ms. There were 322 frames over 30 ms and
104 over the 33.333 ms visible-frame budget; all 2,963 task-batch identity
closures matched. This is not a same-binary comparison with the preceding
`d67ed98b` run, but it is the current directional full-intro evidence after
the retained sparse-checkpoint change.

Audio continuity failed. The last periodic health sample before bounded exit
had accumulated at least 20,438 non-contention underrun sample slots and 5,852
dropped sample slots. A separate phase-armed run closed the same 2,963 frame
identities and localized the tail to `session_execute`: mean/p50/p95/p99/max
were 8.888/5.738/23.912/35.863/100.228 ms, versus 1.861 ms mean planning,
0.429 ms mean commit, and 0.206 ms mean outside-unattributed work. Its absolute
frame times include instrumentation overhead and are diagnostic only.

The exact build source was HEAD `e1f510a8` plus the then-uncommitted
observation patch whose tracked diff SHA-256 was
`6966642a9713d012fea79c2e1ff6fb25a077fd289ea9eee3e39f969566f2091c`; no test
can rederive a later-mutated dirty patch.
Later cleanup or semantic changes require a new PGO build before this can
become final-source certification. No exact A/V synchronization, uninterrupted
audio, red/flame fidelity, or visual-artifact closure is claimed.
