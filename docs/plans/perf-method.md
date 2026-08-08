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

### 6a. Before trusting a lane check, confirm it can FAIL
**A check that returns the same answer regardless of the state it is checking
is not a check.**

Rule 6 says prove the lanes differ. This is the trap one level up: a
*verification* that cannot distinguish the two outcomes it exists to
distinguish. Caught 2026-08-08, mid-run, by the agent that had just written the
check.

The lane-activation check grepped the per-lane logs for
`registered rt64 renderer`. That string **can never appear there**:
`render-benchmark.zsh:222` pipes stdout through a whitelist —
`^\[frame-census\]|^\[fn64-heartbeat\]|render_error|steady idle|^\[wm2000-block-boot\] done`
— and the renderer line matches none of it. So the check would have reported
the string absent in **all four logs, in both lanes**, whether RT64 was active
or not. The dangerous reading is not "could not confirm"; it is glancing at
four identical absences, calling them "no anomaly", and shipping the number.
That is the exact shape of the env gate that fabricated a 4.9x — rebuilt from
scratch by someone who knew that history.

**The ten-second test: run the check against the lane it is supposed to
reject.** Grepping lane A's log for `reference` also returned nothing, which
would have exposed it immediately.

**Knowing this rule does not confer immunity — it recurred within the hour, and
the second instance was nearly the expensive one.** A running A/B driver was
diagnosed *dead* by `ps -eo pid,comm | grep -c "fn64-ab-codegen"` returning 0.
`comm` reports the **interpreter** — every shell script shows as `/bin/zsh` —
so that grep can never match a running zsh script under any circumstances.
Verified: `comm` yields `/bin/zsh` for all shells while `ps -eo pid,args` finds
the script immediately. A replacement driver was written on that false
diagnosis and deleted before it ran; had it started, **two benchmarks would
have shared the CPU and silently overwritten each other through the fixed log
path** — both hazards named two paragraphs above, triggered together.

The general form: `comm` is a proxy for "what is running" and drops exactly the
information that identifies a script. Match on `args` when the identity lives
in the arguments.

The evidence did exist, in the unfiltered `tee` target
`/tmp/fn64-render-benchmark.log`: `[wm2000-block-boot] registered rt64 renderer
(320x240)`. Two consequences worth keeping:

- **That path is fixed and every run overwrites it.** Four sequential runs
  leave activation evidence for only the last. Snapshot it per-run.
- **Two concurrent benchmarks silently corrupt each other's logs**, entirely
  independent of CPU contention — an additional reason the serialization
  protocol is load-bearing, beyond rule 4a.

### 4a. `rustc` is not the only thing that steals a core
`XProtectRemediatorPirrit` — a macOS malware scan — was observed at **97.5% of
a full core** on this machine, unannounced and unrelated to any agent. A
contention check that greps for `rustc`, `cargo` or `wm2000-block-boot` cannot
see it, and no peer's "runs finished" declaration can clear it.

Check by **CPU, over all processes**, not by name:
`ps -eo pcpu,comm | awk '$1>10'`. Anything above ~10% that is not your own
workload is a hazard, whoever owns it. This is rule 15 applied to the machine
itself: observe the state (a busy core), not a proxy for it (a known process
name).

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

### 13. In a shared tree, commit with a pathspec — `git add` is not enough
`git commit` writes **the whole index**, not the paths you just added. When
another agent is working in the same tree it may already have staged its own
files, and your explicit `git add mine.md` then rides along with them.

Earned on 2026-08-07, immediately after this file gained rule 12. The commit
that recorded that rule ran `git add docs/plans/perf-method.md` — one path —
and landed **two** files, because a peer had `vi.rs` staged. It took the call
site `source.cloned_for_scanout()` without `raster/draw.rs`, which held the
only definition. **HEAD did not compile**, and it was pushed. A peer caught
it and repaired it in `199203e`.

Reproduced minimally, so this is mechanism and not conjecture:

```
git add peer.txt      # someone else stages their file
git add mine.txt      # you stage only yours
git commit -m ...     # -> BOTH files land

git commit -m ... -- mine.txt   # -> only mine.txt; peer.txt stays staged
```

Use `git commit -- <paths>`. It commits exactly those paths whatever the
index holds, and leaves a peer's staged work alone. `git add -A` is worse
still and has swept peers' work into unrelated commits three times.

Two corollaries, both of which cost time today:
- **A green test run in a dirty tree proves nothing about HEAD.** The tests
  passed while the definition sat uncommitted beside the committed caller.
  Extract HEAD (`git archive HEAD | tar -x -C <tmp>`) and check there.
- **Do not edit a route file while someone is benchmarking it.** An
  `entrance-to-match.schedule` edit mid-run split an interleaved A/B across
  two different routes — a changed program, not a changed speed. Pin the
  route (`FN64_CONTROLLER_SCHEDULE` at an extracted, hashed copy) and
  announce edits.

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

## The gameplay baseline, 2026-08-07 — this supersedes the menu figures

Everything above this line was measured on a route that idled in menus. The
route now reaches a live match (`5ed7f2c`), and the numbers are materially
worse. **Quote these when asked what 60fps has to beat.**

Route sha `a9e1b25e` (HEAD's schedule), binary `cd3ec985` (clean HEAD), 2.1M
steps, headless, `warmup_gfx=300`, started at load 1.78.

| | gameplay | menu |
|---|---|---|
| mean | **52.79 ms** | 34.85 |
| p50 | 43.05 | 42.64 |
| p95 | 95.18 | 55.82 |
| p99 | 100.30 | 60.58 |
| over budget | 54.4% (6,164/11,321) | — |
| **ratio A** (vs 16.667) | **3.17x** | 2.09x |
| ratio B (wall vs virtual) | 3.155x @ 59.8 Hz | — |

Both ratios agree because the guest is at nominal rate: **no discount here
either.** The gap to close is **36.1 ms per frame**, not 20.9.

Density confirms the cause: 1.44 graphics submits per field against the menu
route's 0.51, **2.8x denser**.

| component | gameplay | menu | ms/field |
|---|---|---|---|
| RSP gfx LLE — raw RDP rasterization | **34.8%** | 31.6% | **16.71** |
| executor self (guest+runtime+guard) | 31.3% | 37.1% | 15.02 |
| VI present (`vi::scanout`) | 8.1% | 11.6% | 3.89 |
| RSP audio LLE | 7.0% | 11.0% | 3.35 |
| gfx LLE other (setup/commit/copies) | 6.4% | 3.6% | 3.06 |
| RSP gfx LLE — RSP interpretation | 6.3% | 1.5% | 3.02 |
| gfx HLE preflight | 6.3% | 3.6% | 3.01 |
| **graphics total** | **53.7%** | 40.3% | |

**The rasterizer alone is 16.71 ms/field — it exceeds the entire 16.667 ms
budget by itself.** An infinitely fast everything-else still misses 60fps.
Everything that scales with submits rose; everything per-field or per-step fell
in share. **The correctness guard is no longer the largest category on a
gameplay route — graphics is, by a wide margin.**

Two caveats, from the measurement rather than added later:

- `max` is 1,452.55 ms and sits **inside** the steady window. The warmup gate
  counts graphics submits, and the match load happens well after submit 300, so
  an arena-load transient lands in the distribution. `p99` is only 1.05x `p95`,
  so the body is well-behaved and that sample is not shaping the percentiles —
  but it does inflate the mean. A gameplay-specific warmup boundary is worth
  having before anyone treats 52.79 as precise.
- **Single run, not an A/B.** Menu-route run-to-run variance was ~±6%, so read
  52.79 as approximately **50–56 ms**.

## RT64 on the gameplay route, 2026-08-07: 1.28x, and it is not enough

Wired in `f74e4e9`, non-default `rt64` feature, `FN64_RENDER=reference|rt64`.
Same route (`a9e1b25e`), 2.1M steps, **two interleaved pairs**, quiet machine.

| | reference | rt64 | change |
|---|---:|---:|---|
| mean | 56.28 / 56.43 | **44.13 / 43.91** | **1.28x** |
| p50 | 43.26 / 44.36 | **27.88 / 28.61** | **1.55x** |
| p95 | 101.88 / 102.11 | 78.40 / 78.11 | 1.30x |
| p99 | 104.04 / 105.51 | 80.14 / 78.74 | 1.32x |
| over budget | 82.9% / 86.2% | 50.0% / 50.0% | -34pp |
| **ratio A** | 3.38x / 3.39x | **2.65x / 2.63x** | |

Ranges are **fully disjoint** and each lane reproduces within 0.5%, far inside
the ±6% this file warns about. The correctness gate passes exactly in both
reps — `gfx_submits=16586`, `audio_submits=11005`, `sp_tasks=27591`,
`vi_interrupts=12008`, `controller_ops=3115`, identical `sim_time`,
`render_error=None`. Same guest program, different host cost.

Three things this settles:

1. **RT64 does not reach 60fps.** 3.38x -> 2.64x. Another **2.64x**, or 27.4 ms
   per frame, still has to come out. It is the largest single win measured and
   it is not sufficient.
2. **The 11.9x recorded in `rt64-throughput-win` does not transfer.** That was
   the *function* lane. The block lane gets 1.28x, now measured on two routes
   rather than extrapolated.
3. **The speedup does not scale with graphics density, which is the surprise.**
   The menu prefix (0.51 submits/field) gave 1.34x; the gameplay route at
   **2.8x the density** gave 1.28x — slightly *less*. If rasterization were the
   whole cost, a denser route should favor the GPU more. It does not, so a
   large share of what RT64 was expected to remove is not rasterization.

p50 improves 1.55x while the mean improves only 1.28x because `max` is ~1,450
ms in **both** lanes: the backend-independent arena-load transient already
noted above sits inside the steady window and drags both means equally. The
median is the more honest figure here.

**Correction to `rt64-on-the-block-lane.md`: headless is NOT present-free.**
`pi::timing` pumps `present_render_backend` at every guest retrace whether or
not a window exists, so blocker C's `PresentMemory::Physical` requirement is
already exercised and satisfied on this lane. Only the post-present blit is
moot.

Two pre-existing bugs had to be fixed to get here, both invisible because the
`rt64` feature is non-default and CI never typechecks it:

- **`fn64-render-rt64 --features rt64` did not compile at all** — 48 errors, a
  visibility regression from the #119 file split. Public API and `#[repr(C)]`
  ABI unchanged. Its suite went 33 -> 71 tests, because 38 were unbuildable.
- **RT64 aborted on WM2000's first VI present.** The C++ shim used **OR** over
  the four H/V fields to decide "window active" where the Rust contract
  (`ViActiveWindow::try_from_registers`) uses **AND**. WM2000's first retrace
  has V_VIDEO programmed (`v=[37,511]`) and H_VIDEO still zero — a normal
  not-yet-programmed state Rust skips. A cross-language contract divergence
  that only a real guest walks into; pinned by a mutation-tested regression
  test using the captured registers.

## What the remaining 2.64x is, under RT64 — it is the guard, not graphics

Profiled at `4f513f0` on route `a9e1b25e`, headless, quiet machine, both
instruments (phase counters + caller-resolved sampling). Reproduces the A/B:
reference 57.19, rt64 43.43 ms/field.

| component | reference | rt64 | share | change |
|---|---:|---:|---:|---|
| **executor self (guest+runtime+guard)** | 16.34 | **18.96** | **45.7%** | **+2.61 worse** |
| gfx LLE — raw RDP | 17.36 | 6.66 | 16.1% | −10.70 (2.61x) |
| RSP audio LLE | 3.91 | 3.95 | 9.5% | — |
| gfx non-LLE (HLE preflight/chunk) | 3.90 | 3.92 | 9.5% | — |
| gfx LLE other (setup/commit/copies) | 3.94 | 3.92 | 9.4% | — |
| gfx LLE — RSP interpretation | 2.99 | 2.95 | 7.1% | — |
| VI present | 3.87 | **1.14** | 2.7% | −2.73 (3.39x) |

**Only two lines moved.** Everything else is unchanged within 1%.

**`RdramView::read_u8` is 44.06% of all samples, and 99.21% of that enters
through `read_snapshot`** — the mutation-journal guard, not graphics. Its four
seams: `osSpTaskStartGo_recomp` 5.97 ms/field, `dispatch_lle_task` 5.89,
`with_render_backend` 3.19, `dispatch_captured_raw_rdp` 3.17.

So **~60% of the remaining per-submit "graphics" cost is guard work at the
renderer seam.** That is why the speedup did not scale with density: graphics
rose 2.82x between routes while RT64's absolute saving rose only 1.34x (47% of
proportional), because the residual per-submit cost is a guard a GPU cannot
touch. RT64's native code does not appear in the self-time table at all — its
work is off-CPU.

The two instruments do not disagree; they slice different axes. Phase counters
bucket by call-tree seam, the sampler by which function burned cycles. Joining
them shows the counters' "graphics" bucket contains ~15.3% of total that is
actually guard.

### The negative result that explains a contradiction

`FN64_FAST_MUTATION_JOURNAL=1` on the RT64 lane measures **+1.15% mean — nothing,
wrong direction** — while the profiler attributes 44% of samples to the guard.
Both are right, and the reason is a reachability bug:

`execution.rs:825` builds an `RdramView`, then calls
`flush_active_host_abi_transaction_with(thread, |physical| view.read_u8(..))` —
a **closure**, not the view. That wrapper (`live_program.rs:2381`) hardcodes
`None`, so at `:2298` the `changed_ranges_from_view` memcmp arm is skipped and
`read_snapshot` runs a **per-byte closure call over the whole 1 MiB watched
region** at every nested-writer entry. **That path is not gated by
`continuous_snapshot_enabled()`**, so the journal switch cannot turn it off.

`execution.rs:565` already calls the `_from_view` variant — the sibling fix the
comment at `live_program.rs:2313-2318` records as worth ~7% of total runtime.
This site was never converted. The fast path exists and is simply unreachable.

Two further costs named and not touched: `dispatch_captured_raw_rdp` allocates
and copies **the entire 8 MiB RDRAM per DPC submission** plus a copyback
(`rsp_commit.rs:1085-1132`), backend-independent so RT64 cannot remove it; and
RT64 adds **+19.5 s of `sys` time** (GPU driver), which is 62% of the
executor-self regression — a real cost of the GPU lane, not an artifact.

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

### `fn64-audio` at codegen-units=256: a real defect, an overstated cure

`examples/wm2000-block-boot/Cargo.toml` sets `[profile.release] codegen-units =
256` (:116) and overrides three packages to `1` — `fn64-recomp-rs` (:139),
`fn64-runtime` (:142), `fn64-abi` (:145). **`fn64-audio` has no override**, so
the crate interpreting **4.13B RSP instructions** builds at 256.

The mechanism is real and verified two ways. `nm` on the rlib shows `run_imem`
(defined `T` in cgu.013) carrying undefined references to
`rsp::ops::dispatch` and `rsp::recomp::decode::decode` — genuine cross-CGU
calls at **8.26B invocations**. And `pub fn decode` (`decode.rs:362`) carries
**no `#[inline]`**; the `#[inline]`s in that file are on the small private
helpers `op`/`rs` at :263/:267. Cross-CGU with no `#[inline]` means LLVM sees
only a declaration — it was **never asked**, rather than having declined.

**But the cure was overstated when this was dispatched, and the correction
belongs here.** `decode` disassembles to **667 instructions**, at two call
sites inside an 873-instruction loop — far past any default inline cost
threshold. Same-CGU placement makes a function an inlining *candidate*, not an
inlined one. The realistic upside is `dispatch` at 126 instructions, not the
one holding 8.3 KB of decode tables.

**Pre-registered prediction, recorded before the A/B runs: 0-2%, most likely
inside noise.** Judge the result against that. It is still worth measuring,
because talking yourself out of a cheap measurement from a disassembler is
rule 1's error aimed at a negative — and inlining is not the only same-CGU
effect (constant propagation into `decode`, whose `pc` argument is derivable at
one site, and register allocation across the call can pay without any inline).

**This is not the recorded LTO dead end, and the reason is shape, not
identity.** That entry is `codegen-units`/LTO/`target-cpu=native` on the
**shards** — 32 generated crates of cold dispatched-through code paying 9
minutes of build for 10%. This is one handwritten crate running a hot
interpreter loop.

**The two RSP items overlap and their results will not add.** There are exactly
2 relocations to `decode` from `run_imem`, and they *are* the double-decode
pair at `interpreter.rs:118-119`. If `codegen-units=1` inlines `decode`, the
redundant delay-slot decode gets cheaper but stays redundant; if the delay slot
is made conditional, one of the two sites disappears. Measure separately; do
not sum.

### Sizing the `run_imem` double-decode before dispatching it

`crates/fn64-audio/src/rsp/interpreter.rs:119` decodes the delay slot
**unconditionally**, before the `match instr` that decides whether anything
needs it:

```rust
let instr = decode(word, pc);
let delay = delay_word.map(|delay_word| decode(delay_word, pc.wrapping_add(4)));
```

**CORRECTION — it is 6 arms, not 4, and the two I missed are the frequent
ones.** The original entry said `Jal`/`Jalr`/`Jr`/`Jump`. Enumerating the
`run_delay(` call sites instead of grepping near the word "delay" gives six:

| line | arm |
|---|---|
| :263 | **`Instr::Branch`** (Beq/Bne) |
| :288 | **`Instr::BranchZ`** (Blez/Bgtz/Bltz/Bgez) |
| :294 | `Instr::Jump` |
| :301 | `Instr::Jal` |
| :307 | `Instr::Jr` |
| :314 | `Instr::Jalr` |

The two omitted arms are the **conditional-branch families**. In compiled MIPS
every loop and every `if` becomes a `beq`/`bne`/`bltz`, while `jal`/`jr` appear
only at call boundaries — and the RSP microcode dominating this workload
(audio and gfx ucode) is loop-heavy. So this is not a rounding error on a rare
arm; the omitted arms are plausibly the **majority** of delay-slot consumers.

**The ceiling below is therefore too high by an unmeasured amount.** The
discard rate is materially under the 80% assumed. Deliberately not substituting
a guess: there is no branch-density count for this workload and inventing one
is rule 3's error. **The measurement needed is a branch-mix histogram, not more
arithmetic** — `FN64_DPC_COPY_CENSUS` (`14ec45a`) already counts retired RSP
instructions, so a two-counter split of "delay consumed" vs "delay discarded"
inside the existing loop would settle it cheaply. Do that before writing any
fix.

Note this makes the change *more* attractive to implement even as the ceiling
falls: decoding lazily inside the six arms that need it is small, local, and
provably equivalent — the delay word is already in hand at every site. A
smaller win for a much smaller change is a better ratio. What it is not is a
step toward 5.84 ms on its own.

The original (wrong) arithmetic follows, kept because the prediction was
pre-registered and should be judged as written:

**But size it before dispatching.** `decode` is a leaf at 7.03% of samples, and
RSP interpretation totals 3.04 ms/field (8.33 ns per instruction):

| decode work removed | mean | ratio |
|---|---:|---:|
| 50% | 21.85 | 1.31x |
| 80% | 21.38 | 1.28x |

So the ceiling is roughly **1.3 ms of the 5.84 ms needed** — worth doing, and
**not sufficient on its own**. Recording the estimate here, before the
measurement, so the result is judged against a prediction rather than fitted to
one. Note the same arithmetic style over-predicted once today (the guard fix
beat its 25.91 ms floor because the defect double-counted itself), so treat
this as a lower bound on the ceiling, not a guarantee.

## The write barrier now SAVES 12.4 ms/field, and the GPU attribution is retracted

Measured 2026-08-08 on the RT64 gameplay route (`a9e1b25e`), barrier on vs off:

| | barrier ON | barrier OFF |
|---|---:|---:|
| mean | **22.96 ms** | 35.32 ms |
| p50 | — | 29.58 |
| p95 / p99 | — | 59.30 / 67.21 |
| ratio A | **1.38x** | 2.12x |
| wall | ~268 s | 422.99 s |

**Turning the barrier off makes the program 1.58x SLOWER.** It is not overhead
to be removed; it saves ~12.4 ms/field by replacing a scanning journal with
MMU-reported dirty pages.

This **reverses a recorded dead end for this route.**
`FN64_FAST_MUTATION_JOURNAL=1` is filed above as measuring **zero** — true on
the old menu route, and **strongly negative here** (22.65 -> 35.32 ms/field).
A dead end is scoped to the route that produced it; re-measure before reusing
one.

**The 17.8% sys-time GPU attribution is RETRACTED.** `/usr/bin/time -l` on the
barrier-off control: 422.99 real / 398.97 user / **36.02 sys = 8.5%**, with
**77 voluntary context switches** over seven minutes, 18 page faults, 0 swaps.
A process blocked on a GPU driver shows neither profile. The barrier A/B bounds
sys at ~1.05 ms/field for the barrier and ~3.0 ms/field elsewhere — a **bound,
not an attribution**, because the barrier-off lane is a different program.

What survives cleanly is a **refutation, not a null result**: `(u+s)/wall` =
1.014 / 1.009 / 0.992 with 66-114 voluntary switches across three runs. The
process is on-CPU essentially 100% of the time. **There is no GPU stall to
pipeline away**, and that line of inquiry is closed rather than merely
unsupported.

Two cautions attached to the control's own numbers: `max=3182.79 ms` is ~90x
p99 and sits inside the steady window — the same arena-load transient flagged
at ~1,450 ms elsewhere — so the median is the honest comparator and the mean
carries one outlier. And its `gfx_submits=16283` is the steady-span count, not
the whole-run 16586; compare like to like before making a byte-identity claim.

### 15. Verify the state you meant to cause, not the call you made
Five instances in one day, 2026-08-07/08, all the same shape and none of them
subtle in hindsight:

- **A green test run in a dirty tree.** The suite passed while the definition
  the committed caller needed sat uncommitted beside it. HEAD did not compile.
- **`kill -STOP` returning 0 while the targets stayed `RN`.** The exit code
  reports that a signal was *sent*, never that it *landed*. The verification is
  `ps -o stat` showing `T`.
- **`pgrep -f "wm2000-block-boot"` matching 21 PIDs** — the benchmark plus idle
  zsh wrappers, `ugrep`, `/usr/bin/time`, and 15 *suspended* rustc whose command
  lines contained the path. A monitor armed on that pattern waited for processes
  that were waiting for the monitor: a self-deadlock. Instantaneous CPU
  (`ps -eo pcpu,comm`, threshold) cannot make that mistake.
- **A guard that worked, under a label that lied.** The check printed the
  offending PID; an unconditional `echo "(empty = none)"` sat beneath it. The
  evidence was right there and the text next to it asserted the opposite.

- **A log whose silence was a buffer.** `nobarrier-run.log` was declared a
  finished short probe because it held 9,543 bytes across a 3-second window and
  its stderr was 92 bytes of a benign notice. It was **still running**: `lsof`
  showed PID 89169 holding it open `1w` at 102.6% CPU, and the stderr grew to
  325 bytes minutes later. **stdout to a file is block-buffered (~4-8 KB), not
  line-buffered** — 9,543 bytes is just past two 4 KB blocks, so a live process
  routinely shows a frozen log between flushes. "Stopped at controller read
  390" was where the last flush landed, not where the guest stopped. This is
  rule 7 one layer down: there the harness printed only on an input edge, here
  the kernel reveals output only on a block boundary. The check that
  distinguishes them is `lsof` — it asks *who holds the file*, not *what the
  file looks like*. File contents are downstream of a buffer; process state is
  not.

Two halves, and the second is the one that nearly slipped through:

1. **Observe the state you intended to cause, not the call you made to cause
   it.** Exit codes, pattern matches, and "the command ran" are all proxies.
2. **Make the label a function of the observation, never a constant printed
   beside it.** A hardcoded verdict is an assertion with no evidence behind it,
   sitting exactly where evidence belongs.

**Corollary — do not hand-shake on observations of quiet.** Three benchmark
misreadings came from an all-clear that was true when issued and stale when
acted on: a run started in the seconds between the check and the resume. A
check and an action cannot be made atomic across agents, so no amount of
re-checking fixes it. Coordinate on *declarations* — "N runs remain", "runs
finished" — not on observed idleness. A suspended build resumes from cache with
nothing lost; a contaminated measurement is silently wrong.

### 14. A rate-limited loop cannot report its own cost
Retract the windowed **~18.8 ms p50** wherever it appears. It is not evidence
the window is faster than the headless lane; it is not evidence of anything.

`shell.rs:1195-1208` holds a cadence: `const FRAME = 16_666_667` ns, a
`next_frame_deadline`, and `ControlFlow::WaitUntil`. When the shell keeps up it
**sleeps**. So `frame_interval_ms` measures the 16.667 ms clamp, not the work —
it reads ~16.7 ms on an infinitely fast machine and on a barely-adequate one
alike. **Quoting it as a per-field cost fabricates a pass of the exact bar it is
being used to test.** ~18.8 ms is simply 16.667 plus scheduler slop: the
signature of a rate-limited loop roughly keeping up.

Its sibling `pump_ms` is honest but measures something else. It brackets
`pump_one_frame()` only, and `pixels.render()` runs later under
`RedrawRequested` (`:975`, `:1184`) — **outside the bracket**. So `pump_ms` is
close to the headless quantity *minus the blit that is the entire reason a
window differs*.

Consequently **the composite windowed cost including presentation is not
instrumented at all.** Neither statistic is it, and no combination of them is
either.

What each is good for: `pump_ms` for guest+runtime cost over a matched span
(reference backend, blit-excluded); `frame_interval_ms` only for *is it keeping
up or falling behind* — where the **fraction of frames that overrun the
deadline** is meaningful even though the median is not. That fraction is the
real playability signal.

The general rule: **before quoting a latency, establish whether the loop
producing it is free-running or clamped.** A clamped loop reports its clamp,
and a clamp set to the target reports success.

## CAVEAT ON EVERY NUMBER BELOW: they are HEADLESS, and RT64 is headless-only

`examples/wm2000-block-boot/src/shell.rs` — the windowed binary — **hardcodes
`ReferenceBackend`** (`:608-612`) and contains **zero** occurrences of
`FN64_RENDER`. The selector `f74e4e9` added went into `src/main.rs` (headless)
only, which has eleven. So:

- **`FN64_RENDER=rt64` on `wm2000-shell` is silently ignored.** It does not
  error and does not warn; the window renders with the software backend
  regardless. Anyone who set it, saw the game run, and concluded "RT64 works
  windowed" was reasonably but wrongly served. A warn-on-ignored-variable guard
  is worth adding.
- **The measured 1.35x is headless-with-RT64.** A player using the window gets
  neither. If RT64's headless 1.28x is the right adjustment and the guard fix
  carries over, the windowed figure lands near **1.73x** — but that is an
  ESTIMATE, and estimating is how this file accumulated its dead ends.
  **Windowed-at-HEAD has never been measured.**
- This also reframes **blocker C** in `rt64-on-the-block-lane.md`. The
  `with_registered_physical_rdram_read` readback concern at `shell.rs:903` was
  never tested because there is no RT64 path in the window to test. It is a
  hypothesis about code that does not exist yet, not an observed failure.

Wiring the selector into the shell is the obvious follow-up, and it is not
free: RT64 renders into its own GPU surface, so the shell's readback and its
`fb_width`/stride assumptions both need revisiting (the capture API
`enable_present_capture()` + `presented_pixels()` already exists).

## PLAYABLE: re-verified on screen at HEAD, 2026-08-08 (`1659c52`)

The last on-screen proof predated both `f74e4e9` (RT64) and `abc7871` (the
1.95x view threading), and both touch code `shell.rs` shares — so "playable"
had become an assertion about a build that no longer existed. Re-checked:

**No regression.** Clock advances `00:07` -> `00:38` -> `00:48` across three
captures 30 s apart, two wrestlers in the ring, both ATTITUDE meters drawn.
5,940+ frames, zero panics, **20/20 distinct `rgba_hash`** across the last 20
heartbeats. A pixel diff puts **10.1% of pixels changed**, concentrated in the
crowd band (**30.4%**) against a near-static ring floor (**1.0%**) — which
proves the animation is real *and* that the camera is stable, so it is not
global drift.

The verdict was deliberately withheld at the versus presentation, because this
file records it as a **looping** cinematic: a frame of it proves rendering, not
gameplay.

**Audio survived, strongest evidence to date:** `ai_buffers=5745`,
`samples=6,023,424`, `nonzero=5,721,107`, `backend_buffers=5745`. Equal counts
means **zero cpal drops**; 48 kHz negotiated from the guest's 32 kHz;
`FN64_NO_AUDIO` unset.

Binary `3021d32f` at `cf234b1`, isolated clean worktree, route `a9e1b25e`,
**REFERENCE backend** — `wm2000-shell` hardcodes it, so this is not the RT64
lane.

**Windowed timing, and it is worse than headless:** steady-state `pump_ms` p50
median **21.1 ms**, with **80 of 80 late heartbeats over the 16.667 ms
deadline — 100%**, against headless's ~50%. The median sits in the same regime
as headless 22.51, which *suggests* the 1.95x carried through, but that is not
a measurement: `pump_ms` excludes the blit and the spans are unmatched. Two
caveats travel with these numbers — the shell has **no warmup gate**, so the
4M-step boot pump (`max=8740 ms`) owns `max` for the whole run, and the binary
carries the `fn64-audio` `codegen-units=256` defect, making them a
**pessimistic floor**.

## Leading explanation of the bimodality: a 30 Hz guest loop (UNCONFIRMED)

An independent read-only diagnosis, ranked by confidence and **not yet
confirmed by measurement**. Recorded because it rules out the obvious
hypothesis by arithmetic and names a cheap decisive test.

**The hypothesis that survives: WM2000 runs its main loop at ~30 Hz** — it
builds and submits its display list once per game frame, one game frame per two
VI fields. The render field absorbs ~2.9 submits; the off-field carries only
audio, present, and the runtime floor. All graphics work is **synchronous
inside the field interval** (`osSpTaskStartGo_recomp`,
`task_dispatch/lifecycle.rs:1015`, dispatch-before-completion at :1067-1073),
so whichever field submits absorbs the entire per-submit cost — no smearing.

Per-submit costs, derived from the measured split: RSP LLE **2.11 ms/submit**
(3.04 ÷ 1.44), raw RDP seam **1.94 ms/submit** (2.80 ÷ 1.44). At 2.88 submits
that is ~11.7 ms, plus the guard clustering on the same fields — **all four
`read_snapshot` seams are per-submit or per-dispatch** — for a total delta near
19.9 ms, giving modes at ~12.9 and ~32.8 ms. That matches p50 16.4 and p95 39.

**"Submit batching, 2 vs 1" is RULED OUT by arithmetic** — and it was the
brief's own leading guess. One extra submit is worth ≈2.11 + 1.94 + ~2.8 guard
≈ **7 ms**, which puts modes at ~19 and ~26: *both over budget*, contradicting
the observed 50% under. A fast mode at 16.4 cannot coexist with a per-field
floor (executor self 12.58 + audio 1.37 + present 1.27) plus even one submit.

Two supporting facts: **1.44 submits/field is itself the tell** — a non-integer
average argues against per-field structure and for per-two-field (1.44 ≈ 2.88 ÷
2). And the menu route shows the same cadence at lower weight, **0.51
submits/field ≈ 1 task per 2 fields** — the same 30 Hz loop with 1-task menu
frames instead of ~3-task match frames.

Also ruled out, each with evidence: **interlaced field parity** (scanout
asserts progressive carries field 0, `fn64-render/src/lib.rs:521-523`; WM2000 is
480x240 progressive), **audio cadence** (0.92 submits/field — present on
virtually every field, cannot alternate), **the double-buffer swap** (a single
VI_ORIGIN MMIO latch; nothing host-side scales with it), and **census
artifact** (per-field normalization of catch-up advances is tested at
`frame_census.rs:161-164`).

### Lag-1 alone is BLIND to period 4 — report lags 1..6 and the raw sequence

Pinned before any real data, with synthetic sequences through the same
estimator, so the reading cannot be restated afterwards:

| sequence | lag1 | lag2 | lag3 | lag4 |
|---|---:|---:|---:|---:|
| period-2 `fSfS` | **−1.00** | +0.99 | −0.99 | +0.99 |
| period-4 `ffSS` | **+0.00** | **−0.99** | −0.00 | +0.99 |
| period-3 `fSS` | −0.50 | −0.50 | **+0.99** | −0.49 |
| contiguous blocks | +0.99 | — | — | — |
| random 50/50 | +0.00 | — | — | — |

**A period-4 sequence reads `lag1 = +0.00` — squarely in the "random, neither
candidate survives" band while being perfectly periodic.** Only `lag2` exposes
it. Period-3 is subtler: `lag1 = −0.50` reads as weak alternation when the real
signal is at `lag3`.

So the decision rule is: strongly negative lag-1 → alternation; strongly
positive → contiguous phase mixture; **near zero → check lags 2..6 before
concluding anything**, because "neither" and "period 4" are indistinguishable at
lag 1.

Report **the raw f/S string first**, then lags 1..6, then the statistic's
verdict. A few hundred characters of eyeball catches a cycle no single
coefficient will. This is rule 6a in statistical clothing: a test that returns
the same answer for "random" and "perfectly periodic with the wrong period" is
not a test for periodicity.

### The decisive test, and the data already exists

`FieldSample` **already retains the cumulative gfx submit count per sample**
(`frame_census.rs:150-155`, recorded at :234-239) — only `report()` discards it.
Two numbers end the question:

1. **Lag-1 autocorrelation of `per_field_ms`.** Strongly **negative** →
   alternation. **Positive** → route-phase mixture (slow fields contiguous).
   Opposite predictions from one number.
2. **Contingency table, over-budget × submit-delta.** If alternation holds:
   delta-0 fields ≈ 5,660 and almost all under budget; delta ≥ 1 fields average
   ~2.9 submits and almost all over.

**If confirmed, the consequence is severe and worth stating now: the render
field must fit 16.667 ms on its own.** ~3 submits of RSP + RDP + guard have to
fit alongside the ~13 ms floor. **No redistribution trick helps — the
off-fields have no work to donate.** The targets become per-submit cost and the
per-field floor, in that order.

## THE DISTRIBUTION IS BIMODAL — and this changes what to work on

The single most important structural fact about the remaining gap, and it was
hiding in plain sight in every census line:

| | A-rep1 | A-rep2 |
|---|---:|---:|
| **p50** | **16.41** | ~16.4 |
| mean | 22.85 | 22.88 |
| p95 | 39.03 | ~39 |
| p99 | 39.75 | ~39.7 |
| over 16.667 | 5,659 / 11,321 (50.0%) | 5,660 (50.0%) |

**The median field ALREADY FITS the 16.667 ms budget.** p95 is 2.34x it. So
roughly half the fields are comfortably inside and the other half miss badly.
This is not a program that is uniformly 1.35x too slow; it is a program with
**two populations of field**.

**Consequence: shaving the mean cannot reach `holds_60fps`.** The
`codegen-units=1` A/B improved the mean 1.6% and moved the over-budget count by
**exactly one field** — 5,659 to 5,660. A uniform few-percent win redistributes
nothing across the line, because almost no field sits near the line.

Every candidate currently queued is a mean-shaver: the double-decode, the
clean-boundary count, the DPC staging copy. They are all worth their cost on
efficiency grounds and **none of them is a path to the bar**.

**The question that matters is now: what makes the slow half slow?** Not "what
is expensive on average". Concretely, before dispatching another mean-shaver,
somebody should:

1. **Split the census by population.** Emit the per-field cost distribution
   bucketed by whether the field crossed 16.667 ms, and diff the phase counters
   between the two buckets. If slow fields carry more graphics submits, that is
   a scene-complexity story; if they carry the same work but more journal
   boundaries or faults, it is an apparatus story.
2. **Check for periodicity.** ~50/50 with a tight p50 and a tight p95 smells
   like alternation or a repeating cycle (every Nth field doing extra work),
   not random spread. `gfx_submits` per field is 1.44 on average — if the slow
   half carries ~2 and the fast half ~1, the cause is submit batching.

Until that split exists, the 5.84 ms figure is misleading: it is the mean's
distance from the bar, and the mean is not what fails.

## THE STANDING BAR: 16.667 ms/field, hardware parity

The goal is **at least as good as original hardware, with the game playable**.
That is `ratio A <= 1.00x` — 16.667 ms per emulated VI field — and
`holds_60fps=true`, on the gameplay route with a real backend.

Where it stands as of `abc7871`:

| | value |
|---|---|
| now | **22.51 ms/field = 1.35x** |
| target | 16.667 ms/field = 1.00x |
| **gap** | **5.84 ms, 26% of current runtime** |
| playable | **yes** — on screen, audio, two players, all verified |
| 60fps | **no** — ~50% of fields over budget |

Two things this bar is NOT, both of which have caused confusion here:

- **It is not the game's frame rate.** N64 VI fields are 60 Hz whatever the
  game renders. A title that draws every other field still requires its
  emulator to deliver 60 fields/sec or wall-clock playback runs slow. The bar
  is per-FIELD.
- **It is not the median.** p50 already fits (16.27 ms). `holds_60fps` needs
  the distribution, not its middle — ~50% of fields still miss, and the tail
  (p95 ≈ 38 ms) is what a player feels.

Progress so far, all measured on route `a9e1b25e`: **19,000x -> 3.38x** (many
sessions) **-> 2.64x** (RT64, `f74e4e9`) **-> 1.35x** (view threading,
`abc7871`).

## RESULT: the guard fix landed at 1.95x — 44.13 -> 22.51 ms/field (`abc7871`)

Two interleaved pairs, RT64 block lane, route `a9e1b25e`, 2.1M steps, quiet
machine. **Ranges fully disjoint, each lane reproducing within 0.8%.**

| | baseline | fix |
|---|---:|---:|
| mean | 44.14 / 43.81 | **22.44 / 22.57** |
| p50 | 28.25 / 27.92 | **16.27 / 16.37** |
| p95 | 79.00 / 78.01 | 38.41 / 38.49 |
| p99 | 80.06 / 78.87 | 39.09 / 39.28 |
| **ratio A** | 2.65x / 2.63x | **1.35x** |

Guest byte-identical across all four runs (`gfx_submits=16586`,
`audio_submits=11005`, `sp_tasks=27591`, `vi_interrupts=12008`,
`controller_ops=3115`, identical `sim_time`, `render_error=None`).

**The median field now fits the budget** — 16.27 ms against 16.667. But
`over_16.667ms` is still ~50% and `holds_60fps=false`: 1.35x is a different
regime from 2.64x, not a solved one.

**The prediction below was BEATEN, and the reason is worth keeping.** The floor
committed before the measurement was 25.91 ms for removing 100% of the guard at
the four seams; measured is **22.51**, 3.40 ms below it. The estimate was built
from the seams' profiled *share*, which undercounted the fix: `flush_host_abi_
transaction_inner` handed `None` **down again**, so
`invalidate_pending_physical_writes_inner` repeated the same per-byte rebuild —
**two full rebuilds of the 1 MiB boot bank per nested-writer entry**, and one
threading change feeds both. A share-based estimate cannot see a defect that
double-counts itself.

Correctness held under the falsification that mattered: the `None` arm lives
*inside* `changed_ranges_from_view`, so passing `Some(&view)` opts into the
attempt only and removes no fallback — an unmapped byte still raises its panic
from the code that owes it. Path equivalence is pinned by
`changed_ranges_from_view_matches_the_copying_path` over randomized contents.

It is also not the rasterizer-hoist shape: judged on mean ms/field, the mean
moved *with* the counter, p50 improved 1.72x, p95 and p99 both roughly halved,
nothing regressed.

**Not re-profiled.** Everything in the table below this line describes the
44.13 ms world and is now stale. Candidate 0 in particular sits on a seam this
fix targeted, so its 3.17 ms/field share has certainly changed and may have
collapsed — re-measure against 22.51 before dispatching anything.

## The 60fps arithmetic: what the guard fix can and cannot buy

Computed from the measured RT64 split, before the in-flight fix reports, so the
expectation is on record rather than fitted afterwards.

The four `read_snapshot` seams total **18.22 ms/field — 41% of the 44.13 mean**:
`osSpTaskStartGo_recomp` 5.97, `dispatch_lle_task` 5.89, `with_render_backend`
3.19, `dispatch_captured_raw_rdp` 3.17.

| guard removed | mean | vs 60fps | vs 30fps |
|---|---:|---:|---:|
| 100% (perfect) | 25.91 | **1.55x** | 0.78x ✓ |
| 50% | 35.02 | 2.10x | 1.05x |
| 25% | 39.58 | 2.37x | 1.19x |

**Even a perfect result does not reach 60fps.** The fix is necessary and not
sufficient: it buys a comfortable 30fps (0.78x) and leaves 1.55x.

What is left afterwards is a **different problem from today's** — the profile
inverts and no single line dominates:

| component | ms/field | share of 25.91 |
|---|---:|---:|
| gfx LLE — raw RDP (RT64 residue) | 6.66 | 25.7% |
| executor self, minus the guard seams | 4.11 | 15.9% |
| RSP audio LLE | 3.95 | 15.2% |
| gfx non-LLE preflight/chunk | 3.92 | 15.1% |
| gfx LLE other (setup/commit/copies) | 3.92 | 15.1% |
| gfx LLE — RSP interpretation | 2.95 | 11.4% |
| VI present | 1.14 | 4.4% |

Graphics becomes **72%** of the remainder, spread over five lines of 1–7 ms.
Another **9.24 ms** must come out, and there is no 16.71 ms rasterizer to
delete this time — it would be five or six separate wins, each small enough
that rule 12 (a large byte count is not a bottleneck) and the counter-versus-
outcome rule both bite hard.

Worth noting `dispatch_captured_raw_rdp` appears in **both** tables: 3.17 ms of
guard at its seam, and separately the untimed 8 MiB copy of candidate 0 below.
Do not double-count them, and re-profile before treating either as available.

## Candidate: 682 journal boundaries per field, 75% of them clean — UNSIZED

7,716,048 boundaries over 11,321 fields is **682 per field**, and the census
reports **75% find nothing dirty** — ~5.8M no-op boundaries. Alongside
5,033,090 `mprotect` calls and 5,033,090 faults (445/field each). That is the
"cheap thing done enormously often" shape that produced the 1.95x win, so it
looks like an obvious target.

**It is not obviously waste, and the framing above is a trap I nearly set for
myself.** Two reasons to size it before dispatching:

1. **The barrier already pays for itself.** Turning it off costs 12.4 ms/field
   (22.96 -> 35.32). A clean boundary is the barrier correctly reporting "no
   guest writes here" — that answer is the product, not overhead.
2. **The 75% figure is telemetry, not a code path.** `CLEAN_BOUNDARIES` is
   incremented inside `write_barrier.rs:1360`'s `note()`, which returns
   immediately unless `enabled()`. It counts how often the barrier finds
   nothing; it does not measure what finding nothing *costs*.

**What would size it:** the per-boundary cost on the clean path specifically —
time the `take_dirty` -> empty-span sequence, not the whole boundary. If a clean
boundary is already a handful of instructions, 682/field is cheap and this
closes. If it re-arms protection or walks a page list before discovering the
list is empty, 5.8M of those is worth attacking.

Until that exists this is an **unsized candidate**, filed here so nobody
dispatches on the raw count. Rule 12 in a new costume: 5.8M is a large number
attached to an unmeasured cost.

## Ranked candidates, with what would falsify each

0. **The 8 MiB RDRAM copy per DPC submission — UNMEASURED, and do not dispatch
   on the byte count.** `dispatch_captured_raw_rdp` (`rsp_commit.rs:1085-1088`)
   does `vec![0u8; staged_end]` plus `copy_from_slice(real)` over the whole
   physical RDRAM on every submission, then copies back. At 16,586 submits that
   is **129.6 GB** over the gameplay route, which looks decisive and **is not
   evidence**. Rule 12 was earned this same day by a 5.92 GB clone with exactly
   this shape that measured **+0.84%, the wrong direction**.
   The RT64 profile bills this seam 3.17 ms/field (7.65% of samples), but that
   figure is **guard work entering through it** — the seam is one of the four
   `read_snapshot` entry points — not the memcpy. The copy's own cost has never
   been isolated.
   Falsify first, cheaply: instrument the allocate-and-copy pair alone under
   `FN64_PHASE_TIMING` and read its self time. If it is small, this is rule 12
   again and the entry stops here. Note also that the in-flight nested-writer
   view fix targets the guard cost billed at this same seam, so **re-profile
   after it lands** — this candidate may shrink or vanish without being touched.

1. **Page size v4.** The argument against smaller pages in the v2 constant's doc
   comment is **stale**: it warned they inflate an O(pages) root, which the v3
   tree made O(log pages). Leaves are 87.5% of digest payload, so 1-2 KiB pages
   could cut them 2-4x. *Falsified if* per-invocation SHA-256 overhead eats the
   saving — the same effect that limited v3's root win to 30 ms.
2. **Commit frequency.** 15,719 root calls over 19,523 steps: a checkpoint on
   ~80% of steps. *Falsified if* the boundaries are load-bearing.
3. **`digest_expected` allocates a `Vec` per call** — 15,719 allocations, 2
   elements each. Trivial, unmeasured, do not do it without a number.

## The verification set, and two traps in it

There was no written list of what to run before claiming a change is verified,
so the set was folklore passed between sessions — and on 2026-08-07 that cost a
red release gate. Run all of these:

```
cargo nextest run -p fn64-abi -p fn64-runtime -p fn64-recomp-rs   # 987
cargo nextest run -p fn64-boot-harness                            # 231
cargo nextest run -p fn64-discover                                # 1069
cargo nextest run -p fn64-render-reference                        # 464
cargo nextest run -p fn64-audio                                   # 345
bash scripts/grade-all.sh                                         # wrong=0 x5
```

**Trap 1 — an omitted crate is an invisible failure.** `8c54a81` (FlashRAM
status) reported "728/728 abi+runtime" and did not run `fn64-boot-harness`,
whose three release-gate tests it had just turned red by changing
`FlashState::default().status` from `0x00` to `0x80`. They stayed red across
every later session that reused the same partial recipe. A test suite you do
not run cannot tell you anything, and the crate you skip is the one that
breaks.

**Trap 2 — `git archive` extracts have a 7-test false-failure floor.** The
"verify HEAD in isolation" recipe elsewhere in this file
(`git archive HEAD | tar -x -C <tmp>`) produces a directory with **no `.git`**,
so the seven tests asserting "this file is tracked by git" cannot pass there.
They fail identically at any commit, healthy or not. That extract is the right
tool for "does HEAD contain a consistent set of source files" (it caught a
missing definition earlier the same day) and the wrong tool for a green/red
verdict. For that, use a real `git worktree` checkout.

## Dead ends — do not retry without new evidence

- **Hoisting loop-invariant span edges out of `raw_pixel_coverage`.** The
  hypothesis was right and the change still loses. `raw_span_edges_at_y_eighth`
  recomputed two 64-bit mul-divs once per *sample* — 8x per pixel — when only
  four distinct sub-scanlines exist and the enclosing loop had already computed
  them. Hoisting into a per-row `RawScanlineCoverage` moved the targeted
  counter **-10.21%** (`gfx_lle_rdp` 94,693.9 -> 85,028.3 ms, ranges fully
  disjoint, hoist sd 593 vs base sd 2776, 6 interleaved runs on a pinned
  route). **The program got 3.25% slower** (census mean 34.85 -> 35.98 ms), and
  `audio_lle` — a phase the change cannot reach — rose **18.35%**
  reproducibly with disjoint ranges, same guest program, byte-identical
  block progress. The three billed phases net **-4.2 s** while the mean rises,
  so the decisive effect is outside them; p50 is unchanged (41.65 vs 41.50), so
  it lives in the tail, not in the per-pixel throughput the hoist targets. Code
  layout / I-cache is the plausible reading and was **not** proven. The source
  is saved with a mutation-tested exhaustive equivalence proof (726 pixel
  positions x 3 primitives x 2 scissors) and is correct — it is worth reviving
  only inside a larger rasterizer change where layout is re-settled anyway.
  **The rule it illustrates: an optimization whose targeted counter improves
  while the program regresses does not ship on the strength of the counter.**
  That is rule 2's error wearing a new costume — reading the instrument
  instead of the outcome.
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
