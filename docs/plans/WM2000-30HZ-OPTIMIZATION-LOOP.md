# WM2000 30 Hz optimization loop

Status: active measurement and implementation plan, 2026-08-23.

## Goal and acceptance bar

The target is the all-Rust `rs + wgpu` play lane at WM2000's native 30 Hz.
One drawn frame is measured directly between consecutive committed VI swaps;
it is not inferred from a pump population or from the game's nominal cadence.

A candidate reaches the performance bar only when all of the following hold:

1. the post-warmup swap-gap histogram is at least 97% gap two;
2. drawn-frame p95 is at most 33.333 ms;
3. the over-budget fraction is zero in each certification window;
4. the result repeats for ten consecutive clean runs on a quiet machine;
5. the required framebuffer/differential gates remain unchanged.

The p95 and zero-over-budget requirements intentionally make “reliable 30 Hz”
stricter than a mean below 33.333 ms. Until all five conditions hold, the
status is “not verified.”

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
2. **Profile.** Capture a CPU profile from that exact build and window. Use the
   `cpu-profile` table and its recorded image load address; do not use `sample`,
   shared kdebug stack fragments, or inferred ASLR slides.
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

1. Finish and validate omission of diagnostic-only GPU TMEM projections on the
   CPU presentation lane.
2. Finish and validate parallel row execution for byte-identical VI dither
   restoration.
3. Capture a fresh combined-build profile; prior profiles are selection history,
   not authority for the new binary.
4. Attack the largest remaining exclusive cost, currently expected among VI
   restoration, blend/combiner work, texture sampling, and raster traversal.
5. Reconsider command-level validation only when the fresh profile attributes a
   material cost to it. Move stable invariants into types or once-per-command
   validation; never delete guest-visible RDP semantics to meet the budget.

The queue is deliberately provisional after item 2: each fresh profile, not
this document, chooses the next optimization.

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
