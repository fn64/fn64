# WM2000 performance: the 30 Hz budget and the open post-fix question

**Status corrected 2026-08-22.** This document previously framed WM2000 as a
60 fps problem and compared one field's work with a 16.667 ms rendered-frame
budget. That was wrong for the title under test: WM2000 renders at 30 Hz, so
one rendered frame gets two 60 Hz fields, or **33.333 ms**. The correction is
based on the measured live-shell diagnosis retained in
`docs/RT64-WM2000-FRAME-RATE-MEASURED.md` and the scheduling change in commit
`54a994c9`. No post-fix frame-rate measurement exists yet.

## Current finding: scheduling, not a throughput ceiling

The pre-fix observation was **28 rendered frames/s**:

| quantity | value |
|---|---:|
| time per rendered frame | 35.714 ms |
| WM2000 budget | 33.333 ms |
| measured miss | **2.381 ms (7.14%)** |

That refutes the old 26-34% gap derived from p95 pump time. A pump is a 60 Hz
field-sized host slice, not a WM2000 rendered frame, so treating its p95 as a
drawn-frame throughput requirement mixed two different denominators.

The measured defect was **(c) scheduling**, not aggregate throughput and not
submission distribution: a DP deadline strictly inside the current field was
being observed only by the following wall-paced pump, delaying guest progress
by an entire 16.667 ms field. Throughput was close enough to the 30 Hz budget
that a residual 7.14% miss may still exist, but it did not explain why a
nominal two-field rendered frame needed a third retrace pump.

The competing blocking/serialization hypothesis was also rejected. The wgpu
raw-DPC coordinator is synchronous and CPU-side
(`crates/fn64-abi/src/task_dispatch/rsp_commit.rs:427`). In the measured
slow-pump breakdown, ordinary CPU rasterization was **28.0%** and the TMEM
loop **8.2%**; no fence, mutex, wait, or other named candidate exceeded 28%.
Those shares do not support one dominant serialization stall.

## What commit `54a994c9` changed

Landed 2026-08-22: when the guest is quiescent, a device deadline strictly
before the next VI edge is serviced in the current pump; the VI edge itself
belongs to the next wall-paced pump. The rule is characterized by
`timing::tests::a_quiescent_pump_services_full_sync_before_the_next_vi_edge`.
It adds no sleep and no pacing constant.

The evidence chain is explicit:

- The public RDP Programming Manual defines the Sync Full to DP-interrupt
  contract (`docs/DESIGN.md:2219-2220`).
- fn64's existing compatibility policy schedules a raw FullSync DP event one
  cycle after synchronous publication
  (`crates/fn64-abi/src/pi/mmio.rs`, `start_live_dp_full_sync`); this is a
  deterministic policy, not a hardware-latency claim.
- Pinned RT64 at `f0728a2` advances and enqueues its current workload at
  FullSync before its workload thread consumes it (`rt64_state.cpp:1750-1755`,
  `rt64_workload_queue.cpp:881-907`).

Together these establish that the sub-field completion is real work for the
current guest drain, while the next VI edge remains the boundary of the next
wall-paced pump.

## What is not yet known

**The post-fix frame rate has not been measured.** The scheduling defect is
identified and the ordering rule landed, but this document does not claim the
30 Hz target is now met. If the pre-fix 28 fps observation exposes a residual
throughput gap after remeasurement, it is approximately **7.14%**, and the
measured next targets are **rasterization** first and **TMEM** second. Do not
scope either optimization until the post-fix route has been measured.

## Historical measurements that remain valid, but are not the live ceiling

The following numbers were measured before this correction and are retained
for provenance. Their old interpretation is superseded.

**2026-08-20, older block-lane measurement.** With graphics subtracted, two
repetitions reported host-side totals of **21.554 ms** and **21.361 ms**, with
subtraction and named-row sums agreeing within 0.046 ms. The largest rows were
translated guest code (9.528/9.462 ms) and the mirror boundary
(8.848/8.797 ms). Those values were compared with 16.667 ms to produce the
stale 1.29x/1.28x claim. They neither exceed WM2000's 33.333 ms rendered-frame
budget nor describe today's shell route: the mirror boundary was subsequently
reduced to approximately 0.001 ms on that route. They remain historical lane
measurements, not a renderer-independent performance ceiling.

**Older block-lane RT64 decomposition.** A 27.68 ms render-field sample
attributed 14.91 ms to graphics (8.30 ms rasterization, 5.09 ms RSP
interpretation, 1.45 ms staging), 8.23 ms to recompiled guest CPU, 1.68 ms to
invalidate writes, 0.98 ms to audio LLE, and 0.16 ms to the mirror boundary.
That decomposition remains a dated measurement of that lane. In particular,
its 5.09 ms RSP cost must not be transplanted to the current shell route,
where the retained measurement is 0.315 ms.

**Historical renderer ratios.** The 11.9x and 1.28x figures in
`docs/plans/rt64-on-the-block-lane.md` compare RT64 with the reference
renderer; they are not Rust-vs-C CPU results. The 1.28x observation also
predates the `abc7871` nested-writer fix. Neither ratio answers the 30 Hz
scheduling question above.

## Measurement rules that still apply

- State the actual backend. `render-benchmark.zsh` does not export
  `FN64_RENDER`, so an unlabeled run can silently exercise the software
  rasterizer.
- The release gate covers determinism and byte identity, not performance; no
  automated performance-regression gate currently protects these numbers.
- Report profiler overhead. This project has measured a profiler inflating
  its own subject by 26.4%, and a predicted +0.029 ms change landing at
  +1.62 ms. Predictions are not evidence.
