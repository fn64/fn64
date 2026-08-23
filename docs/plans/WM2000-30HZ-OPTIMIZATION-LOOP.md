# WM2000 30 Hz optimization loop

Status: active measurement and implementation plan, 2026-08-23.

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
window.

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
