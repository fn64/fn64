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

`FN64_SCHEDULER_FRAME_DEBT=1` arms a default-off audio/video scheduling A/B.
The legacy deadline rule preserves cadence only while the next event-loop turn
starts less than one field late. A heavy pump can finish in that window, then
its separately dispatched redraw/presentation pushes the following turn past
one field; legacy scheduling reanchors there and drops the otherwise cheap
retrace that produces the next audio task. The A/B retains that fact as a
typed, renderer-independent one-field debt after the pump has fully returned.
The next event-loop turn clears the debt before advancing exactly one ordinary
`next_vi_deadline`; it then reanchors from a fresh post-pump/redraw-request
instant and cannot rearm itself. There is never a second pump in one
`about_to_wait` callback, so close/input/redraw events retain an opportunity
between retraces and guest retraces, audio tasks, and RSP ownership are never
coalesced. The heartbeat reports cumulative `scheduler_debt` retained,
catch-up, reanchor, and maximum-debt counters; `max_debt` is structurally at
most one. Unset or `0` preserves the prior deadline rule and zero counters.

Audio health now partitions the compatibility `underrun_sample_slots` total
into `empty_ring` and `lock_miss` slots. The realtime callback still uses
`try_lock` and silence on contention; the split changes only evidence, not
callback or producer ownership.

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
phase deltas; the v3 receipt folds those deltas between consecutive swaps
under `task_cpu_phase_frames` and `abi_task_phase_frames`. Legacy input remains
valid and reports optional sections as unavailable rather than turning absent
counters into zero-cost claims.

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
