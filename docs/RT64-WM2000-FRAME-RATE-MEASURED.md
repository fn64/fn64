# WM2000 frame rate: where the time actually goes

Everything here is MEASURED on the interactive shell route
(`scripts/play-wm2000.sh`), **`FN64_RENDER=wgpu`**, rs recompiler lane, no
`--features rt64`. Every number states its renderer, because
`render-benchmark.zsh` does not export `FN64_RENDER` and a graphics figure
without its renderer beside it is not a result.

Instruments: the shell's own `pump_census` (already existed; closure-checked
against `counter_tree::TREE`) plus a new `session_phase_census` that splits
`gfx_lle_rdp_ns` into the four phases of the raw-DPC session path. No external
profiler: `samply` and `cargo-instruments` are not installed here, and the
in-tree census infrastructure is both cheaper and closure-checked, which a
sampling profiler is not.

## The budget, stated correctly

**WM2000 renders at 30 Hz, so a drawn frame gets TWO field budgets: 33.333 ms**,
not 16.667. Measuring this game against a 60 Hz bar overstates the gap by 2x.
The pump census's own `over_budget` column is taken against 16.667 ms and is
therefore a FIELD statistic, not a drawn-frame one -- do not read it as "35% of
frames are late".

When `FN64_PUMP_CENSUS_SEQUENCE` is armed, the census also emits one joined
`[wall-cadence-seq]` row after the following pump starts. It attributes the
exact pump-to-pump wall interval to the prior pump index, including its
scheduled deadline, start debt, reanchor decision, pump and redraw costs,
intended wait, wake overshoot, and the residual outside those measured regions.
Separate `[wall-swap-seq]` rows carry swap-to-swap wall time under the ending
swapped pump's index; that backward-looking interval must not be correlated
with the cadence row covering the ending pump's following interval. The last
pump is intentionally pending because no following start exists, while a
completed final swap interval remains reportable. The v4 receipt exposes these
rows under `wall_cadence`; old logs remain readable and report it unavailable.
This join is the arbiter for a rare long event-loop/wake stall: subtracting
unrelated percentile maxima is not.

Schema v5 additionally carries `present_dependencies` when
`FN64_PRESENT_CACHE=observe` or `1` is paired with the full pump sequence. Its
separate `[present-dependency-seq]` stream is indexed by the pump whose work
has just completed, so unlike the forward wall interval it includes the
bounded final pump. A complete stream has exactly one contiguous receipt per
measured pump. The Observe/Suppress comparator requires 1,600 receipts in each
lane and exact equality of their canonical Cacheable identity or Uncacheable
reason. The identity binds overscan/zoom policy and a SHA-256 computed during
the single live-framebuffer traversal. FNV, cache generation/invalidation,
probe time, hit, and redraw disposition remain lane-specific diagnostics; the
receipt reports probe-time distribution separately from pump/present work.

## 2026-08-22 pacing diagnosis: sub-field DP completion was rounded to a field

This supersedes the hypothesis that the renderer's submissions merely needed
to be spread more evenly across pumps. The live path had two distinct facts:

1. **Measured number:** after the diagnostic GPU draw was removed, the slow
   pump mean was 28.58/28.63 ms; later exact raster changes reduced it to
   26.32/26.49 ms with p95 31.8/32.1 ms. The contemporaneous rigid sequence
   was 3.5 ms, 4.4 ms plus swap, and one graphics pump; before the later raster
   reductions those three costs totalled about 41 ms per swap. A reported 28
   rendered frames/s is 35.71 ms per rendered frame, only 7.1% over the
   33.333 ms budget -- not 26-34%.

   **Inference:** throughput was close to the budget and could own a small
   residual miss, but it did not explain why a nominal two-field frame needed
   a third retrace pump.

2. **Source fact:** fn64's raw FullSync policy schedules DP completion exactly
   one cycle after renderer publication
   (`crates/fn64-abi/src/pi/mmio.rs`, `start_live_dp_full_sync`). The ABI's
   `next_device_deadline` documentation already states that a pump advancing
   only in field-sized steps delivers such mid-slice completions a full field
   late. The headless `GuestDrain` already services the exact device deadline
   before the next VI edge; the live shell did not. Pinned RT64 likewise
   advances and enqueues the current workload at FullSync before its independent
   workload thread consumes it (`rt64_state.cpp:1750-1755`,
   `rt64_workload_queue.cpp:881-907`, pin f0728a2).

   **Inference:** the dominant defect was scheduling, not submission
   distribution. Renderer work completed synchronously, then the scheduler
   waited until the next 16.667 ms host pump to observe a one-cycle DP event.
   The fix services exact non-VI device deadlines while the current retrace
   pump is quiescent, resumes the guest work they wake, and stops at the next
   armed VI edge. It also advances later pumps to the fabric's exact VI
   deadline so sub-field advances cannot drift the VI cadence.

The blocking hypothesis is also rejected for this wgpu route. Its production
raw-DPC coordinator is explicitly synchronous and CPU-side; the direct timing
split after the GPU-draw gate measured 5.29 s in `execute_raw_dpc_inner`, of
which 3.88 s was ordinary CPU rasterization. The later 9.72 s slow-pump
breakdown found rasterization at 2.72 s (28.0%), the TMEM loop at 0.80 s
(8.2%), and no named candidate above 28%; that is accumulated CPU work, not one
fence, mutex, or FullSync wait.

The deterministic characterization is
`timing::tests::a_quiescent_pump_services_full_sync_before_the_next_vi_edge`:
a one-cycle device deadline must remain in the current pump, while a deadline
equal to the next VI edge must remain for the next pump. This is an ordering
rule derived from the existing device schedule, not an assertion of WM2000's
current submission counts.

## CONFIRMED: the ranked breakdown, per drawn frame

Per graphics task (= per drawn frame), 600-pump window, 212 graphics tasks:

| bucket | ms/drawn frame | share |
|---|---:|---:|
| **graphics (`gfx_lle_ns`)** | **58.60** | **82.6%** |
| — of which `execute` (CPU rasterizer) | 47.81 | 67.4% |
| — of which `plan` (decode + triangle setup) | 10.61 | 15.0% |
| — of which finalize + commit | 0.23 | 0.3% |
| VI presentation (`vi_present_ns`) | 10.42 | 14.7% |
| non-graphics host side (guest CPU + apparatus) | 1.93 | 2.7% |
| **TOTAL** | **70.94** | **2.13x the 33.333 ms budget** |

The RSP graphics microcode (`gfx_lle_rsp_ns`) is **0.315 ms**, audio is
negative in the slow population, and `exec_mirror_ns` and
`resume_invalidate_ns` both read **0.001 ms or less**.

Closure: the pump census residual is **0.0–0.2%** on both populations. The
session phase census attributes 11615.1 ms across 4169 submissions; the pump
census's `gfx_lle_rdp_ns` row over its 212 slow pumps is 12344.8 ms. **Two
instruments built from different counters on different clocks, agreeing to
94%**, with the residual explained by the session census also counting the
warmup pumps the pump census discards.

## CONFIRMED, and it corrects a widely-cited figure

`docs/plans/perf-method.md:3234` states that with graphics set to **exactly
zero** the host side is still **21.55 ms = 1.29x budget**, and concludes that
**an infinitely fast renderer still misses 60 fps**. That was true when it was
written. **It is not true on this route today**, and the difference is not
subtle:

| row | perf-method.md (block lane, pre-fix) | this route (shell, wgpu) |
|---|---:|---:|
| mirror boundary | **8.85 ms** | **0.001 ms** |
| invalidate writes | 2.02 ms | 0.000 ms |
| translated guest code + rest | ~10.7 ms | ~1.93 ms |
| **host side with graphics at zero** | **21.55 ms (1.29x)** | **12.35 ms (0.37x)** |

The mirror boundary — the single largest row in that table — was fixed by
`8109435` (`docs/plans/NEXT.md:9-16`, one line, 14.3 → 29.0 fps). This
route's `exec_mirror_ns` of 0.001 ms is that fix, measured. So the old
conclusion should no longer be quoted as a live constraint:

> **With graphics at zero this route would run at ~81 fps, comfortably inside
> both the 30 Hz and the 60 Hz bar. The renderer is not merely the largest
> cost; on this route it is very nearly the ONLY cost.**

The 21.55 ms figure is left cited rather than deleted, because the error is
the instructive part: it was measured on a lane whose dominant row has since
been eliminated, and it was still being cited as a reason not to expect a
renderer fix to matter.

## What this makes the target, and what it does not

`execute` — the CPU rasterizer — is **67.4% of a drawn frame** and is the only
bucket whose removal changes the answer. `plan` at 15.0% is a real second
target. Everything else together is 17.7%, and **eliminating all of it
entirely would not reach the 30 Hz bar.**

Specifically NOT targets, each measured rather than assumed:

* **VI presentation, 10.42 ms.** Large in absolute terms, but **flat across
  the fast and slow populations** (3.662 vs 3.714 ms/pump, a +0.05 ms delta).
  It is a fixed per-pump cost, not part of the tail, and the frame rate is set
  by the tail.
* **The whole-RDRAM staging copy.** `dpc_calls` reads **0.00** on this route:
  WM2000 goes through `try_dispatch_raw_dpc_via_session`, which does not stage
  a whole-RDRAM image at all. The copy census exists and is armed; the path is
  simply not taken. An 8 MiB-per-submission cost that was worth chasing on the
  legacy path does not exist here.
* **The RSP graphics microcode, 0.315 ms.** `docs/plans/rt64-on-the-block-lane.md`
  measures RSP interpretation at 5.09 ms/field on the BLOCK lane, and a
  RSP→Rust recompiler exists out-of-tree. On THIS route the same bucket is
  0.315 ms — 16x smaller — so wiring that recompiler in would be worth at most
  0.3 ms here. That is a real difference between the two routes and should be
  re-measured before anyone spends the effort on the shell's account.
* **The guest CPU.** 1.93 ms/drawn frame. Not the problem on this route.

## A candidate that is NOT yet evidence

`production.rs`'s composition loop re-materialises the whole colour target
after every command:

```rust
accumulated = Some(completed.device_bytes().device_bytes().to_vec());
```

At 480x237 that is ~227 KiB per command, and a packet carries many commands,
so the byte count is large. **A large byte count is not a bottleneck** —
perf-method rule 12 was earned by a 5.92 GB clone whose complete elimination
measured **+0.84%, the wrong direction**. This has NOT been counted, because
`fn64-render-wgpu` does not depend on `fn64-abi` and inverting that layering
for a probe would be a worse change than the one being measured. It is
recorded as a suspect with the reason it is only a suspect. **Count it before
restructuring it.**

## The honest remaining gap

At 70.94 ms per drawn frame the route is **2.13x the 30 Hz budget** —
~14 fps of drawn frames, presenting at the ~26 Hz the retrace counter
reports. Closing it needs the rasterizer, and nothing else in the measured
decomposition is large enough to matter:

* Halving `execute` alone: 47.06 ms, still 1.41x.
* Halving `execute` AND `plan`: 41.76 ms, still 1.25x.
* `execute` and `plan` both to zero: 12.35 ms, 0.37x — 81 fps.

So the rasterizer must get roughly **3x faster**, not 20% faster, for this
route to hold 30 Hz. That is a real target rather than a hopeless one: the
per-pixel path calls `pixel_coverage` and `attribute_sample` separately per
pixel (each scanning the same subsamples), and every admitted triangle is
textured, so every covered pixel also pays a `sample_point`. Those are
measurements someone else's card already owns; this document exists so that
card can start from a number instead of a hypothesis.

## Scope limit, stated plainly

**This is a boot/menu/attract-mode measurement, not an in-match one.** The
runs reached VI swap ~1,260; the match goes live at swap ~6,336, and reaching
it needs a human on a pad. The bottleneck IDENTITY (rasterization dominates,
apparatus does not) is stable across every window measured and matches the
26 Hz the owner reports, but the absolute triangle counts in a match will
differ. Anyone extending this should drive the shell to a match by hand and
re-run the same two censuses.
