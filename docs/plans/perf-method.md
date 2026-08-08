# Performance method

Written 2026-08-07, after a session that made **nine wrong calls** on this
question and shipped six real wins. The wins all came from handing an agent a
measurement. The wrong calls all came from handing one a hypothesis.

## CLOSED: the choppy audio is the speed deficit. There is no audio bug.

Investigated 2026-08-08 after the owner played WM2000 and reported choppy audio
at the title screen. **Outcome: no defect on the audio path. The stream is
starved because the emulator runs at ~46% of real time, and the audio shortfall
and the render-field shortfall are ONE measurement, not two problems.**

Record this before the mechanism, because it was the expensive part: **the
figure that opened the investigation was wrong by 2x.** "91.5% of real time"
came from reading `AudioOutputStats::samples` as frames when it counts i16
channel samples. Corrected, delivery is **45.7%**. See rule 21; the wrong number
is left standing in `shell-frontend-gaps.md` with a correction beside it.

### The arithmetic, which closes exactly

**Guest side — healthy in virtual time.** On the byte-identity gate route,
`sim_time=18776001537` is **200.28 virtual seconds** at 93.75 MHz:

| | | |
|---|---:|---|
| `vi_interrupts=12008` | **59.96 fields/virtual-s** | nominal 60 ✓ |
| `audio_submits=11005` | **54.95 buffers/virtual-s** | |
| ratio audio/VI | **0.9165** | not 0.5 — no alternate-buffer drop |

**Buffer geometry.** A measured AI buffer is 1048 i16 = **524 stereo frames =
16.38 ms of audio at 32 kHz** — one VI field. So the guest produces almost
exactly one field of audio per field, and needs **61.0 buffers/wall-second** to
sustain realtime.

**Host side — the deficit.** The owner's session delivered 1,290,576 samples ÷ 2
= 645,288 frames over 44.1 s:

| | |
|---|---:|
| delivered | **14,632 frames/wall-s** |
| against 32,000 Hz | **45.7% of realtime** |
| as buffers | **27.9/s against 61.0 needed** |

**27.9 buffers/s is the emulator delivering VI fields at 27.9 Hz instead of 60.**
The audio deficit *is* the field-rate deficit — the same number reached from two
independent directions. 45.7% of realtime and the render field's 2.15x budget
are the same phenomenon; **anyone treating them as two problems will
double-count the work.**

The guest's programmed rate is **inferred from buffer geometry** (524 frames
landing on a 16.38 ms field boundary implies ~32 kHz) rather than read from
`AiFrequencyChanged`. Provenance noted so the next person can tighten it; it did
not need tightening to close this question.

### What was ruled out, and how

Three factor-of-two candidates, all eliminated — the near-50% figure beside a
confirmed 30 Hz render period made a units/cadence error the leading
hypothesis, ahead of guest speed:

| candidate | verdict | evidence |
|---|---|---|
| **resampler applies 32→48 ratio to samples where it means frames** | eliminated | `BandlimitedResampler::process` divides by `channels`; `step` advances a frame-coordinate phase. **Mutation-tested**: injecting the exact 2x error failed 3 of 6 tests |
| **stereo/mono mismatch** | eliminated | a mono misread would make a buffer 1048 frames = 32.8 ms = two fields; measured is 524 = one field |
| **per-field cadence dropping one of two buffers** | eliminated | `audio_submits/vi_interrupts = 0.9165`, not ~0.5 |

The resampler is correct and loses no material: equal rates pass through
byte-identically, and the windowed-sinc path is pinned by six tests including
image-band suppression.

**The three video fixes that landed the same day (RT64, `AutoNoVsync`, 30 Hz
frame pacing) did not help the audio, and could not have** — but not because
audio is unrelated to them. They reduced render cost without moving
wall-versus-virtual enough to change the field delivery rate, and field delivery
rate is what audio production tracks.

### The instrument that hid it, now fixed

`examples/wm2000-block-boot/src/shell.rs` — the played binary — read
`AudioStreamHealth` but printed `underrun_samples` / `late_callbacks` /
`max_callback_gap_us` **only on SPIKE lines**. The routine heartbeat showed
`ai_buffers`/`samples`/`nonzero`/`backend_buffers`, all of which read clean
under starvation. Equal buffer counts were taken as proof of health for hours.

The heartbeat now also prints the host counters and a computed
**delivered-frames-per-wall-second against the guest's programmed rate**
(`audio_rates()`, not a hardcoded 32 kHz), with the channel-sample unit named in
the label. See rules 21 and 22.

### Consequence

**There is no audio fix that is not "make the emulator faster."** A resampling
or time-stretching workaround would hide the shortfall at the cost of timing
fidelity, and is not proposed. This line closes and points back at the
**19.17 ms render-field** problem, which remains the single bar.

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

**PARTIALLY SUPERSEDED 2026-08-08 — the measurement stands, the explanation
does not.** The reachability bug below was real and `abc7871` fixed it, but the
flag still measures nothing *after* that fix: **−0.14 ms/render-field over
three interleaved pairs with the barrier ON** (see the corrected section under
"The write barrier now SAVES 12.4 ms/field"). So the bug was **not the whole
reason** this read as null. The dominant reason is that with the barrier on,
the ungated scheduler-mirror reconcile arms one step ahead of the gated call,
leaving it an empty dirty set to compare. Keep the diagnosis below as the
history of a genuine defect; do not keep it as the explanation of this null.

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

~~This **reverses a recorded dead end for this route.**
`FN64_FAST_MUTATION_JOURNAL=1` is filed above as measuring **zero** — true on
the old menu route, and **strongly negative here** (22.65 -> 35.32 ms/field).~~

**RETRACTED — this sentence was mine and it is wrong.** The table above is a
**barrier** A/B (`FN64_MPROTECT_BARRIER` on vs off). **The flag is not varied
anywhere in that experiment.** I took the barrier-OFF number, 35.32, and
asserted it as the *flag's* cost. One measurement doing duty in two different
experiments.

Measured properly — flag on vs off, **barrier ON in both lanes**, 2.1M steps,
route `a9e1b25e`, RT64, headless, quiet machine, **three interleaved pairs**:

| pair | lane A (flag off) | lane B (flag on) | delta |
|---|---:|---:|---|
| rep 1 | 36.08 | 35.59 | **−0.49 ms (−1.36%)** |
| rep 2 | 35.71 | 35.91 | **+0.20 ms (+0.56%)** |
| rep 3 | 35.72 | 35.58 | **−0.14 ms (−0.39%)** |
| **mean** | | | **−0.14 ms, sd 0.35** |

**The deltas span both signs.** Within-lane spread across reps (A 0.37 ms,
B 0.33 ms) is as large as the between-lane delta, which is the definition of a
result inside noise. Off-field moved +0.03 / −0.03 / +0.16 — nil, as expected
for a per-dispatch cost. Guest byte-identical on all seven counters in all six
runs, so the flag does not change the emulated program.

**Do not quote rep 1's −0.49 alone.** An earlier revision of this entry did,
before replication, and it reads as a small win; the second pair reversed its
sign. **−0.14 ms is 0.75% of the 19.17 ms the render field needs.**

**Rule 6a — the lane check CAN fail, and it passed.** `barrier_served`
increments only in `stats::note()`, reached only from `barrier_spans()`
(`live_program.rs:372`, `:424`), both *below* the gate on the block path.
Boundaries fell **7,716,048 → 5,911,979 (−1,804,069, −23.4%)**, reproducing
bit-for-bit across both proof reps. It fell but **not to zero** — pre-registered
as the required outcome, because the ungated mirror and host-ABI paths still
call the same comparison. A drop to zero would have falsified the mechanism
below; an unchanged count would have meant the flag never reached the gate and
the A/B was invalid.

**So the two historical "measures zero" entries were RIGHT, and for a reason
this file already half-states**: with the barrier on, `matches_view` compares
only MMU-reported dirty pages, and the ungated mirror reconcile runs the same
comparison on every step **and arms on its match path**
(`write_barrier.rs:1246-1250`: *"The reconcile arms immediately on its match
path, so the common case reads once"*). By the time the gated call runs, the
dirty set is empty and the second comparison is already nearly free. **The
barrier absorbed the cost.** No reachability bug is needed to explain it.

That has a further consequence for the `+1.15%, nothing` entry above, which
attributes its null result to the `None`-passed-where-a-view-was-in-hand bug
that `abc7871` fixed: this measurement is **post-`abc7871`** and still reads
~1%. So the reachability bug was **not the whole explanation** — barrier
absorption is the more likely one, and it survives the fix.

A dead end is still scoped to the route that produced it. But **check first
whether the experiment varied the thing the entry names** — that is the error
here, and it is worse than a scoping mistake.

**Why the second comparison is free, traced in source rather than profiled.**
`host.rs:361-382` runs `mirror_guest_running_thread` **before** `run_one_step()`
on every scheduling step; its own comment says *"this is a FULL watched-region
journal reconcile ... and it runs on every step."* That reaches
`reconcile_before_dispatch_from_view` (`live_program.rs:2205-2225`), which is
**not gated by `continuous_snapshot_enabled()`** and arms the barrier on its
match path. The gated call at `runners.rs:1033` then asks a freshly-armed
region, gets an empty dirty set, and matches trivially. Removing it removes
almost nothing — which is exactly what three pairs measured.

**Correctness: the comment's justification is wrong, but the conclusion is
roughly right, and the flag is not the lever anyway.** The comment at
`live_program.rs:2162` says the comparison "only asserts that no undeclared
write occurred, which write attribution already guarantees." Attribution does
**not** guarantee that: `write_barrier.rs:52-57` lists `as_mut_slice`, the DMA
paths, the RSP/renderer slices and raw `RdramPtr` stores as bypassing the
declaration path, and `live_program.rs:2088-2094` records generated C shims
writing guest memory below every attributed store — with a mitigation
(`snapshot_for_host_shim` / `declare_host_shim_writes`) that has **zero
non-test callers**. What saves the flag is not attribution but the *ungated*
mirror: it re-runs the same comparison one step later, so an undeclared write
is still caught, with at most one step of delay. **Do not cite the 0x0009b0b3
incident as evidence against this flag** — that failure belongs to the reverted
write-queue gate and to the baseline-*advancing* read, both of which
`live_program.rs:2545-2560` distinguishes from this one in as many words.

**Zero test coverage.** Full-tree grep finds three non-doc occurrences of
`FN64_FAST_MUTATION_JOURNAL`, all inside `live_program.rs` itself. No gate, no
script, no CI job runs the flag-on lane.

**One separable defect worth fixing regardless.** The gate returns *above*
three O(1) assertions — `assert_not_poisoned`, the `sealed` assert, and
`PENDING_ATTRIBUTED_EXECUTABLE_WRITES == 0` (`live_program.rs:680-692`). Only
the memcmp was meant to be skippable. Move the gate below the asserts.

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

## CONFIRMED: WM2000 renders at 30 Hz. The target is the render field, not the mean.

Measured 2026-08-08 at `c2caafe`, RT64, route `a9e1b25e`. The 30 Hz hypothesis
below is **confirmed**, including the number it predicted.

**The raw sequence, 400 consecutive steady-state fields from field 2000:**

```
SfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSf
```

**Perfect period-2 alternation, zero defects across all 400.** Visible, not
inferred from a coefficient. Representative fields: `2000: 38.29 ms gfx=3` /
`2001: 8.76 ms gfx=0` / `2002: 37.16 ms gfx=3` / `2003: 8.35 ms gfx=0`.

**Contingency table — essentially a clean partition:**

| | fast | slow |
|---|---:|---:|
| submits = 0 | **5,657** | 0 |
| submits > 0 | 4 | **5,659** |

**100.0% of slow fields carried a submit; 0.1% of fast fields did.** Four
fields out of 11,321 break the pattern. Mean submits when nonzero: **2.88 slow
vs 1.00 fast** — the independent diagnosis predicted ~2.9 vs ~0 and ruled out
2-vs-1 by arithmetic; measured is **2.876 vs 0.001**.

**The two populations:**

| | count | mean | p50 | p95 | share of wall |
|---|---:|---:|---:|---:|---:|
| fast | 5,661 | **8.89** | 8.80 | 9.63 | 19.9% |
| slow | 5,660 | **35.84** | 36.99 | 38.33 | **80.1%** |

**The program spends 80% of its time in 50% of its fields**, and the fast half
runs at **0.53x budget** — less than half of it. The off-field was never the
problem.

Note the lag table read `lag1 = -0.550` with odd lags negative and even
positive at equal magnitude — the textbook period-2 signature, but understated
because each mode has internal variance (the slow mode spans 36.5-38.3). **The
string showed a perfect signal the coefficient rated 0.55.** That is why the
raw sequence is reported first.

### The 5.84 ms figure was the wrong denominator, and by a factor of ~3.4

Every mean-based sizing in this file **understates the requirement**, because it
averages across an off-field that already has 7.8 ms of headroom.

| framing | requirement |
|---|---|
| mean (what was quoted all session) | remove **5.69 ms** from 22.36 |
| **reality** | render field **35.84 -> 16.667 = remove 19.17 ms** |

**A uniform saving of X ms delivers only ~X/2 to the render field**, because
half of it lands on a field that did not need it:

| uniform saving | reaches the render field | share of the 19.17 needed |
|---|---:|---:|
| 0.37 ms (the `codegen-units` win) | 0.18 ms | **1.0%** |
| 1.00 ms | 0.50 ms | 2.6% |
| 2.00 ms | 1.00 ms | 5.2% |

So `c2caafe` — a real, range-disjoint, correctness-neutral win — is **1% of the
way to the bar**, not the ~6% its mean delta implied. Every queued candidate
must be re-sized against 19.17 ms on the render field, and **only the fraction
of its saving that lands on a submitting field counts.**

This also flips which candidates matter. A **per-submit** saving pays entirely
into the population that needs it; a **per-field** saving pays half of it away.
The guard's 3.8x concentration on render fields makes it a per-submit cost, not
the uniform overhead it was filed as.

### RETRACTED: the "graphics RSP is 40% slower than audio" gap never existed

Claimed: the graphics RSP path runs at 11.6 ns/instruction against audio's
8.3 — same interpreter, 40% slower on one path. An agent was dispatched to
explain it. **It is an artifact of my own arithmetic: one numerator, two
denominators.**

Both figures are `gfx_lle_rsp_ms`, the graphics-branch RSP wall time. From
`/tmp/census-run.log`:

| | | |
|---|---|---|
| 35,886.807 ms ÷ **2.978e9 gfx steps** | = **12.05 ns** | quoted as "graphics 11.6" |
| 35,886.807 ms ÷ **4.131e9 gfx+audio steps** | = **8.69 ns** | quoted as "audio 8.3" |

Ratio 1.387 against the claimed 11.6/8.3 = 1.398 — **agreeing to 0.7%**, which
is the signature of a denominator error rather than two real measurements.

**The audio interpreter's ns/instruction has never been measured at all.**
`AUDIO_LLE_RSP_NS` exists (`task_dispatch/lifecycle.rs:87`) but is accumulated
only at `rsp_commit.rs:399`, not on the path taken at `:130`, so it reads
`0.000` on every run. There was no audio figure to compare against.

This is rule 2's cousin and rule 10's exact shape: **two quantities that look
comparable, aren't.** Rule 10 already says "state both ratios or you have said
nothing" about wall-vs-virtual; the same discipline applies to any derived
per-unit figure. **Before comparing two rates, state both numerators and both
denominators explicitly** — if they share a numerator, there is only one
measurement.

### MEASURED: both RSP paths cost 11.26 ns/instruction. The gfx line is LARGE, not SLOW.

The retraction above left the audio rate unmeasured. It is now measured, which
turns a "we compared unlike quantities" into a positive result.

`rsp_commit.rs:128` armed its per-chunk timer with `gfx_started.map(..)` —
`None` on the audio branch — so `rsp_execution_ns` stayed 0 there and
`audio_lle_rsp_ms` printed `0.000` on every run. The accumulation at `:384`
was always correct; the value was always zero. **One line**, gated by the same
`FN64_PHASE_TIMING`, so an unset run still takes no clock read (`7af71f8`).

Measured 2026-08-08, one run, **same timer, same function, same route**
(`a9e1b25e`, RT64, headless, 2.1M steps, quiet machine at load 2.72):

| path | RSP wall time | instructions | **ns/instruction** |
|---|---:|---:|---:|
| graphics | 33,511.987 ms | 2,978,070,834 | **11.253** |
| audio | 12,991.325 ms | 1,153,167,300 | **11.266** |
| | | **difference** | **−0.1%** |

**The interpreter costs the same per instruction on both paths**, to three
significant figures. This is a same-run, same-timer comparison, so it is
immune to the between-run drift that moved the graphics rate 12.05 → 11.25
(−6.6%) across two runs of the same binary lane. **The gfx-vs-audio difference
is ~60x smaller than the run-to-run noise** — that ordering is what makes the
equality the robust finding rather than the coincidence.

**All four candidate explanations are eliminated, because there is nothing to
explain:**

| candidate | verdict |
|---|---|
| different instruction mix (vector-heavy gfx) | cannot be read off this comparison — the *rates are equal* |
| chunking / re-entry cost | 67,989 steps per `run_imem` entry; per-chunk setup is amortized ~68,000-fold |
| memory access patterns | no rate difference to attribute to them |
| guard work inside the RSP timer | the timer brackets `run_imem` alone on both branches |

One caveat worth stating so it is not over-read: **this does not prove a
vector op costs the same as a scalar op.** It proves the two *microcode
workloads* average to the same cost per instruction. A `Vu` arm really does
pay an extra cross-CGU `dispatch()` plus 8-lane element work
(`interpreter.rs:229`), and audio ucode is also vector-heavy on the RSP — the
likeliest reading is that both workloads are vector-dominated, not that
vectors are free. Settling that needs an opcode-mix histogram, not this table.

**What this changes about the 6.08 ms line.** It is **not a slow interpreter**;
it is **526,161 instructions per render field at a uniform rate**. That
reframes the lever entirely:

- **Making the interpreter faster** pays across *all* 4.13B instructions, gfx
  and audio alike — a genuine but uniform per-instruction win, and the
  double-decode is the only sized candidate there (and is a mean-shaver).
- **Executing fewer instructions** is the only thing that touches the render
  field specifically. That means HLE-ing the graphics microcode rather than
  interpreting it, which is a correctness/architecture question, not a perf
  micro-optimization.

Stated plainly, because it closes a line of inquiry: **there is no interpreter
defect to find here.** The honest framing is that the RSP line is large, and
"large" is a different problem from "slow" with a different set of fixes.

Two further facts from the same run, recorded because they were free:

- **Audio LLE is 95.6% interpretation** (12,991 of 13,583 ms), against
  graphics LLE's 39.2%. Audio is essentially pure `run_imem`; graphics carries
  the RDP seam alongside it.
- **`audio_lle_rsp_ms` was a dead instrument for its entire existence.** Rule
  6a's shape one more time: a counter that reads `0.000` on every run is
  indistinguishable from a counter measuring something genuinely zero, and
  nobody checked which it was.

**Instrument perturbation, reported as required:** this run's mean was
**24.21 ms/field** against the 22.36–22.41 instrumented band — **+8.0%**,
*outside* the ±6% run-to-run band. Do **not** attribute that to the one-line
timer change: the internal split moved in a way that edit cannot cause —
`gfx_lle_rdp_ms` rose 33,610 → 51,605 (**+53.5%**) while `gfx_lle_rsp_ms`
*fell* 6.6%. Arming an `Instant` on the audio branch cannot move RDP time.
Unexplained, flagged rather than rationalized, and it does not touch the
ns/instruction result, which is a within-run ratio. Guest byte-identical
throughout (`gfx_submits=16586`, `audio_submits=11005`, `sp_tasks=27591`,
`vi_interrupts=12008`, `controller_ops=3115`, `sim_time=18776001537`,
`render_error=None`).

### CLOSED: async dispatch buys 0 ms, and WM2000 is already `HleOptimized`

Two proposed tracks, both closed on evidence already in the tree.

**Track A — asynchronous RSP/RDP dispatch: ceiling is 0 ms, not 13.4.**

The deferral it asks for **already exists**. Every LLE path ends in
`start_live_rcp_task_with_latency(..., pre_ucode_steps + lle.steps)`
(`pi/mmio.rs:828` → `fabric_ops.rs:182`), scheduling the SP event at
`now + sp_latency` with DP one cycle later. **The guest-observable interrupt is
already delayed by the real instruction count** — 179,553 cycles per task
against 1,563,624 per field, i.e. 11.5% of a field, which is the physically
correct amount. The comment at `lifecycle.rs:1070-1073` describes *host*
ordering, not guest visibility.

A genuine chunked async path also exists — `HLE_RENDER_CONTINUATION` +
`advance_hle_render_task` (`lifecycle.rs:152-224`), resuming at each host
scheduling boundary and handling mid-task SIG0 yield. **That is precisely the
architecture Track A proposed, already built.** WM2000 never enters it because
it takes `NeedsLle` first.

**The decisive reason is stronger than "not enough": the off-field's 7.8 ms is
not host idle time.** `GuestDrain::advance_to_next_device_event`
(`fn64-boot-harness/src/lib.rs:1317-1340`) *jumps* the virtual clock to the
next deadline when the guest quiesces — it does not spin. The off-field costs
8.89 ms of real host work. There is no wall-clock slack to donate, so moving
rasterization there converts 35.84/8.89 into ~22.4/22.4: **the same total wall
time, both fields still missing 16.667.** Async dispatch redistributes zero
host work.

**Track B — instruction volume: the premise was inverted. WM2000 already runs
`HleOptimized`.**

`main.rs:894-902` selects `LleAccuracy` **only** in
`generated_runner_rsp_audit_mode`, which requires argv to be exactly
`[exe, GENERATED_RUNNER_RSP_RUNTIME_ARGUMENT_V1]`. The benchmark passes no
argv, and `HleOptimized` is the thread-local default. The brief's claim that
WM2000 selects `LleAccuracy` was **false**.

It is not recognized, and the counters prove a partition rather than a sample:
`gfx_ms` accumulates at both the HLE chunk seam (`setup.rs:454`) and the LLE
seam (`rsp_commit.rs:386`), and the census reads **`phases=33172` against
`tasks=16586` — exactly 2x**. Every task is HLE-preflighted, rejected with
`NeedsLle`, and re-run through LLE.

**Populating the ucode catalog would not fix it.** `xbus_dpc=16586`,
`dram_dpc=0` — 100% XBUS, so RDP commands live in DMEM produced by the
microcode as it runs; there is no display list in RDRAM to decode. And
`imem_replacements=60763` = **3.66 IMEM overlay swaps per task**: the ucode
reloads its own text mid-task, so a single 4 KiB digest cannot identify it.
`microcode.rs:11-12` already says F3DZEX2 is "named but unadmitted because
those allowed sources do not specify its family-specific continuation and
branch commands" — admitting it is a clean-room RE project under an MIT
constraint, not a config change.

**Correction to this file: the "gfx non-LLE (HLE preflight/chunk) 3.92
ms/field" line is stale by ~1,300x.** The guard fix (`abc7871`) collapsed it:
2,213 µs/call before, **1.88-2.36 µs/call after**, reproducibly across two
independent 2.1M-step runs — 0.003 ms/field. **The HLE preflight is free.
Nobody should target it.**

## MEASURED: the biggest single item is `verify_live_words` — 3.50 ms, and it is a build flag

**Correction to the section below.** The remainder is **not** primarily the
`match pc` dispatch, and the codegen rewrite it implies is **not** the right
first move.

`examples/wm2000-block-shards/build.rs:330` passes **`verify_live_words: true`**.
It is emitted at the top of `'run: loop` (`emit/mod.rs:602-616`), *before* the
`match` — so **once per guest instruction** it does a bounds check, an
`EXPECTED_WORDS` index, and **a full `mem.load_w` guest load** through
`read_mmio_word` → `backing_offset`.

Ablation on real emitted code (512-instruction runner, censused WM2000 mix:
15.9% loads / 12.6% stores / 71.5% ALU), seven variants interleaved in one
process, 5 samples each, 4 independent runs, each ablation verified textually
distinguishable first:

| component | ns/instr | ms/render field | % of the 19.17 gap |
|---|---:|---:|---:|
| **`verify_live_words` total** | **3.10** | **3.50** | **18%** |
| — the guest load | 1.66 | 1.88 | 10% |
| — call + `AotMiss` construction | 1.44 | 1.63 | 8% |
| `advance_cop0_random` | 0.61 | 0.69 | 4% |
| `post_straight_instruction_exit` | 0.26 | 0.29 | 2% |
| residual (`match` + real work) | 4.02 | 4.54 | 24% |
| **total as emitted** | **7.99** | **9.03** | **47%** |

**Three of my named suspects are near-zero and should be dropped.**
`advance_cop0_random`'s two integer divides are **0.61 ns** — LLVM
strength-reduces them. The write-boundary TLS access is **0.26 ns**, and
measured alone it flipped sign across reps (−0.93/+1.18/+1.09): a layout
artifact, not a cost. And the per-load redundancy claim **does not hold** —
`translate_data_address` returns early for `DirectVirtual`/`DirectPhysical`, and
the 32-entry linear TLB scan is only reached for `Mapped` (KUSEG) addresses.
**WM2000 is KSEG0 and never enters that loop.**

**My "the emitter already knows block boundaries" premise was also wrong.**
`emit/mod.rs:619` is `for (index, instr) in instrs.iter()...` — one arm
**unconditionally per instruction**. The doc comment at :25-30 describes the
*whole-function* emitter, not the dense shard runner WM2000 uses. Over the
1 MiB boot copy: 262,144 arms emitted, **46,011 genuine block entries (17.6%)**,
mean block **5.7 instructions**, so 82.4% of arms are removable — but that
bounds only the 4.02 ns residual, most of which is the guest's real work.

### Sizing: the gate is 78% of the win for a fraction of the cost

| change | render field | ratio A | % of gap |
|---|---:|---:|---:|
| **gate `verify_live_words` off** | 35.47 → **31.97** | **1.92x** | **18%** |
| + block-structured emission | → 30.98 | 1.86x | 23% |

The gate needs **no emitter logic change** — `verify_live_words` is already a
`bool` on `DenseBankShardInput`. Flipping `build.rs:330` touches the emitter's
*caller*, avoiding the certified-source digest move (`emit/mod.rs` **is**
certified, via `lib.rs:76`'s `generated_runner_emitter_source_receipt_v2`).

**Block-structured emission should wait.** A further 0.99 ms (5% of the gap)
does not justify a certified-source edit, a 32-crate rebuild, and a
restructuring that must preserve the delay-slot rule exactly.

### The correctness argument, with the gap stated honestly

A **second detector is already live** — verified: `execution.rs:88` installs
`classify_live_executable_write` as the guest-write-boundary observer, backed by
`EXECUTABLE_WRITE_BOUNDARY` (`recomp-rs/runtime/host.rs:279`) and consumed by
`post_straight_instruction_exit` at every instruction boundary. Plus
`activate_for_fetch_with_digest` re-digests on every activation, so stale code
cannot execute in the un-resident case either.

**But this is defence-in-depth removal, not redundant-work removal, and must be
argued as such.** `write_barrier.rs:52-57` lists paths that bypass the
declaration channel (`as_mut_slice`, raw `RdramPtr` stores, some renderer
slices); `verify_live_words` is the belt-and-braces detector for exactly those.
The `sim_time=18776001537` byte-identity gate is what confirms no undeclared
writer is being relied on in practice.

The existing `verify_precompiled_instruction_word` dead end does **not** cover
this: it is about fixing the check *inside* `fn64-recomp-rs`, and its stated
reason — that crate "cannot see the barrier" — is precisely why the gate belongs
at the `build.rs` call site.

## THE ANSWER: the recompiled code is an interpreter, and that is the whole gap

The question was whether fn64 misses optimizations the original hardware or its
1999 compiler had. **It does, and it is basic-block compilation itself.**

`crates/fn64-recomp-rs-codegen/src/emit/mod.rs:14-20` documents the emitted
shape in its own words:

```
'run: loop { match pc {
    0x…00 => { <ops> ; pc = 0x…04; }     // straight-line fall-through
    0x…10 => { <ops> ;                    // a branch site
    0x…44 => { <ops> ; return; }          // jr $ra
```

**A PC-keyed dispatch table re-entered per guest instruction.** That is the
shape of an interpreter written in Rust, not of compiled code. Per instruction,
unconditionally: a `match pc` dispatch, `advance_cop0_random` (two integer
divides), a TLS access for the write boundary, and — where `verify_live_words`
is on — **a full guest memory load to re-read the instruction word before
executing it.** Per guest *load*: segment classification runs **four times**,
the alignment check three, bounds validation twice, and the 32-entry TLB is
scanned linearly with **no break-on-match**. Registers never live in host
registers across a block: every operand is `ctx.r(N)` into memory, and `ctx`
escapes into the memory path, so LLVM must spill the register file around each
load.

**The arithmetic is the whole story:**

| | |
|---|---|
| remainder | 11.96 ms over 1.13M guest instructions |
| per guest instruction | **10.6 ns** |
| on a ~3.5 GHz host | **~37 cycles per guest instruction** |
| one N64 instruction at 93.75 MHz | **10.7 ns** |

**We spend 10.6 ns emulating an instruction the console executed in 10.7 ns.**
Modern hardware is ~37x faster per clock and the per-instruction dispatch loop
gives all of it back. A well-formed static recompilation should approach 1-2
cycles per guest instruction, not 37.

**So "why isn't modern hardware enough" has a clean answer: it is enough, and
we are not using it.** Threading, batching and caching are real but secondary —
together worth roughly a third of the 18.80 ms gap. This is the other two
thirds, and it is **codegen quality, not a correctness tax**: the guard is 24%
of the field, the barrier pays for itself, and the RSP interpreter is
large-not-slow at a defect-free 11.25 ns.

**Cost of fixing it is the real obstacle.** Every file in `fn64-recomp-rs/src`
is a certified source, so a codegen change moves identity digests, and rule 8
prices it at 32 crate rebuilds. This is a program, not a patch.

### CLOSED: the RSP cannot move to a worker thread — the deadline depends on the result

Stronger than the existing "async buys 0" entry, which rests on the empirical
claim that the off-field has no host slack. This one is structural.

`fabric_ops.rs:179-181`: *"Schedule completion after a **measured** amount of
synchronous RSP work."* The latency argument is
`pre_ucode_steps.saturating_add(lle.steps)`, and **`lle.steps` is the retired
instruction count produced by `total_steps` (`rsp_commit.rs:403`) — it does not
exist until interpretation has finished.** The virtual deadline is *computed
from* the work, so the scheduler must block on the worker immediately. You buy
the handoff cost and nothing else. Even with infinite host slack it returns
zero.

Two further blockers: all state on the path is `thread_local!` (`HOST`,
`RENDER_BACKEND`), so a worker would see **different empty instances** — a
wrong-answer failure, not a deadlock. And `RunToken` (`thread.rs:178`) is
`pub struct RunToken(())`, auto-`Send`/`Sync` — **reentrancy protection on one
call stack, not serialization.** Its own doc says the guarantee holds *"since
nothing in this crate spawns a second OS thread"*: an assumption, not an
enforcement. The type would not catch the mistake.

Threading the RSP correctly requires predicting instruction counts before
executing them. That changes what the emulator is.

### CLOSED: the instruction budget is not a free parameter, and steps are not budget-bound

Re-tested 2026-08-08 on the **rendering** route under RT64, post-guard-fix, with
the mirror's 8.43 ms/field known — conditions that did not exist when the
original dead end was recorded. **The entry is strengthened, not reversed**, and
for a reason nobody had established.

`FN64_BLOCK_INSTRUCTION_BUDGET` defaults to 4096 and the benchmark never sets
it. Three 300k-step probes, every counter normalized by its lane's `sim_time`:

| counter | 4096 | 8192 | 16384 |
|---|---:|---:|---:|
| `vi_interrupts` | 1.0000 | **1.0000** | **1.0000** |
| `audio_submits` | 1.0000 | 0.9999 | 1.0001 |
| `gfx_submits` | 1.0000 | 1.2161 | **1.3228** |
| `sp_tasks` | 1.0000 | 1.0869 | 1.1300 |
| `pi_started` | 1.0000 | 0.9173 | **0.8742** |

**The two exact `1.0000` rows are the evidence**, not decoration: they prove the
normalization is sound, so the rows that move are moving for real. **Graphics
submits rise 32% per unit of virtual time while PI DMAs fall 13% — opposite
directions.** A program running faster moves its counters together or not at
all. This is a *different emulation*. Run queues differ at equal step count too:
`[17,3,1]` / `[17,1]` / `[6,1]`.

**And the mechanism fails independently.** Steps would not have halved anyway: a
2x budget buys only **1.178x** guest work per step, 4x buys 1.291x (7,960 →
9,378 → 10,274 cycles/step against a budget-bound ideal of 1x/2x/4x). **Most
dispatches already end on something other than budget exhaustion**, so the
budget does not control step count. Either finding alone closes it.

The clamp was verified inactive first, so this is a real result rather than a
null one for an unrelated reason: `canonical_instruction_limit`'s only non-test
setter is gated on `FN64_BLOCK_EXPECT_GUEST_INSTRUCTIONS`, which the benchmark
never sets.

**Consequence for the bar: the mirror-halving line comes off the projection.**
The mirror is per-step, step count has no cheap knob, and reducing per-step
apparatus now requires either a **cheaper mirror** or a change to **when** the
watched region is verified. That is the architectural option, not the
incremental one.

One more rule-6a trap caught in passing: `grep "registered rt64 renderer"` on
the *binary* returns 0 and means nothing — the string is built at runtime from
`{active_renderer}` (`main.rs:903`). RT64 is genuinely linked (40 symbols), and
the `rt64` arm panics rather than falling back, so a mislabeled
reference-backend number is not possible on this lane.

### 20. Test the verifier on this machine, not just its logic

The lane gate built to enforce rule 19 **crashed instead of verifying**:

```
find: Can't parse date/time: @1786223147
```

`find -newermt "@<epoch>"` is **GNU syntax; BSD `find` on macOS rejects it**. The
gate had been tested against synthetic inputs and correctly failed all three
bad-lane scenarios — but that testing exercised its *logic*, never its
*portability*. On the real machine it aborted before counting anything.

**It failed safe, which is the one good thing here**: no binary was stashed, so
nothing unverified reached the benchmark. A gate that crashes is far better than
one that passes on error — but it still cost a cycle, and a differently-written
gate would have silently returned zero matches and reported a clean lane.

Portable form, verified on this machine:

```sh
STAMPREF=$(mktemp /tmp/lane-stamp.XXXXXX)
touch -t "$(date -r "$STAMP" +%Y%m%d%H%M.%S)" "$STAMPREF"
find <dir> -name runner.rs -newer "$STAMPREF"
rm -f "$STAMPREF"
```

The general rule: **a verification script is itself code that can be wrong, and
synthetic-input testing does not cover the environment.** Run it once against
the real tree before trusting it to gate a measurement — and prefer a
construction whose failure mode is a crash over one whose failure mode is an
empty result set, because empty reads as "clean" (rules 18 and 19).

### 28. Check WHICH POPULATION a candidate pays into before sizing it

Earned 2026-08-08. The strongest-looking optimization target of the session was
**57% of a fast field and 16% of a slow one**, and every measurement against it
returned a null — because the fields it improved were the ones already inside
budget.

The census report states the rule in its own output, and it is worth quoting
verbatim because it is the whole lesson in one sentence:

> *"A saving in a row that is large on the fast row and small on the slow one
> pays into the population that already has headroom."*

On a bimodal workload — WM2000 is `SfSfSfSf`, ~50% of fields over budget — **a
share computed over both populations tells you almost nothing about whether a
candidate can move the bar.** The mirror boundary is the worked example:

| | fast field | slow field |
|---|---:|---:|
| mirror | 4.16 ms (**57.0%**) | 9.13 ms (**16.2%**) |

Read the blended number and it is the biggest line in the program. Read the
slow row — the only population that fails 16.667 ms — and it is a sixth of a
field that is 2.8x over. **Deleting it entirely could not have moved the bar,
and the A/B confirmed exactly that.**

**So: before sizing any candidate, split it by population and read the SLOW
row.** A candidate that is large only on the fast row is not a 60fps candidate
at all, however large its blended share. This compounds with rule 2 — an
inclusive counter read as a peer, *and* a blended share read as a slow-field
share, will both point at the same wrong target.

### 27. The benchmark harness DISCARDS any report tag it does not know

Earned 2026-08-08 by losing a completed full-route census run to it. **This
will eat the next instrument too**, so it is filed separately from the
contamination incident it was discovered during.

`render-benchmark.zsh` pipes the binary through

```
grep -E '^\[frame-census\]|^\[fn64-heartbeat\]|render_error|steady idle|^\[wm2000-block-boot\] done'
```

**Any `atexit` reporter outside that allowlist is invisible in the saved log** —
the new `[mirror-reconcile]` census and the *pre-existing*
`[mprotect-barrier]` stats both vanish. The unfiltered copy goes to
`/tmp/fn64-render-benchmark.log`, a **fixed path that every run overwrites**,
so by the time the omission is noticed the next run has already destroyed the
evidence. A full-route census was run, completed, and lost exactly this way;
the figure that ended up in this document came from a 20k-step re-probe
instead.

**The instrument looked broken when it was working perfectly.** That is the
expensive part: the first hypothesis was a dead counter, and diagnosing *that*
is what caused the contamination in rule 26.

**The remedy, and it is the rule:** when adding a counter that reports at exit,
**save the unfiltered stream to a per-run path** (`"$BINARY" 2>&1 | tee
"$OUT/run-N.full.log" | grep ...`). Do not extend the allowlist and assume that
suffices — the next counter after yours will hit the same wall — and never rely
on the shared `/tmp` tee surviving into the next run.

This is rule 18's family once more: a pipeline that cannot distinguish *"the
counter printed nothing"* from *"the counter's output was filtered away"*
reports both as absent, and absent reads as "the feature is broken".

**The sharpening, from a second incident the same hour: the filter also hides
the diagnostics that would tell you data is missing.** An `FN64_EXECUTOR_SPLIT`
pair was run without `FN64_FRAME_CENSUS_POPULATIONS`, which the split report
actually depends on (`frame_census.rs:1320` calls it from the *population*
report). The census code handles this correctly and prints

> `[executor-split] NOT ARMED (FN64_EXECUTOR_SPLIT unset). The executor_ns
> decomposition is absent, not zero -- do not read this as 'the phases cost
> nothing'.`

— textbook rule-18 discipline, distinguishing absent from zero. **That warning
was never seen, because it rides on `[frame-populations]`, which the filter
also drops.** Two 25-minute runs completed with no split data and no
explanation of why.

So an allowlist filter does not merely lose results; it **defeats the
safeguards built to report their absence**. A well-instrumented program behind
a lossy pipe is indistinguishable from a broken one. Capture the full stream
first, filter for reading second — never the reverse.

**A safeguard on a filtered channel is not a safeguard.** That is the part
that generalizes past this harness, and it is a design rule, not a workflow
tip:

> **A warning must travel on a channel that cannot be configured off
> independently of the data it is warning about.**

Here the warning rode `[frame-populations]` while warning about
`[executor-split]`, so one filter decision silenced both the data and the
notice that the data was missing. The same shape appears whenever a diagnostic
is emitted at a lower log level than the thing it guards, on a debug-only
channel, or behind a second feature flag: the configuration that suppresses the
signal suppresses the alarm with it. **Route alarms to stderr, to an always-on
channel, or to a non-zero exit — anywhere the reader cannot accidentally
deselect them while selecting for results.**

**Related gate trap, and it is a design smell rather than only a user error.**
Producing the `executor_ns` decomposition requires **three** environment
variables:

```
FN64_PHASE_TIMING=1  FN64_EXECUTOR_SPLIT=1  FN64_FRAME_CENSUS_POPULATIONS=1
```

The third is **neither named by nor implied by** the other two — it is required
only because `executor_split_report` happens to be called from inside
`population_report` (`frame_census.rs:1320`). Nothing about "arm the executor
split" suggests "also arm the population census", and arming two of three
yields a full, healthy-looking run with the decomposition silently absent.

**Arm all three, or the run is wasted.** A reasonable future cleanup is to have
`FN64_EXECUTOR_SPLIT=1` imply the population report, since the split is
useless without it — a gate whose stated purpose cannot be achieved by setting
it alone is a gate with a missing dependency, not a user error.

### 26. A diagnostic probe is a benchmark: cheap-feeling commands slip past the gate

An operational failure from 2026-08-08, and a genuine gap in the rules as
written. **Rule 4 says "quiet machine" and rule 9 says "no rebuild beside a
benchmark" — neither says a diagnostic probe is a benchmark.**

**A 20-second probe contends exactly like a 25-minute one.** An agent that had
just *deferred a `cargo` build* to avoid disturbing a running measurement then
launched a 20,000-step probe of the same binary **while the next measured run
was in its steady window** — and had to discard that run. The rule it had
internalized was "don't build during a measurement"; the rule that applies is
**"don't run anything during a measurement."** A probe feels like a lookup
rather than a workload, and that feeling is the whole failure: cheap-looking
commands slip past a gate that expensive-looking ones respect.

Before running *any* command while a benchmark is live, ask what it costs the
scheduler, not what it costs you to type. If a diagnosis genuinely cannot wait,
kill the measurement first and restart it — a discarded run you chose is
cheaper than a contaminated one you have to detect later.

**What made this one land: the probe was diagnosing a different trap.** The
counter it was chasing had not failed — its output had been filtered away by
the harness (rule 27). So a false "broken instrument" signal produced an
urgent-feeling diagnostic, and the urgency is what walked it past the gate.

**Traps compound: the second mistake is made while fixing the first.**
Demonstrated twice in one session, both times by an agent actively holding the
relevant rule in mind — once contaminating a run while diagnosing a filtered
counter, once wasting two 25-minute runs on a missing third gate while fixing
the filter. Believing you are being careful is not a control; it is the state
you are in immediately before both of these.

**That is the argument for a checklist over vigilance.** Vigilance is a
resource that debugging consumes, and it is lowest exactly when the next
irreversible action is taken. Before starting a measured run, verify
mechanically: machine quiet, **every** required gate armed, output captured
unfiltered, and nothing of your own about to run. Four checks, thirty seconds,
and they do not degrade under pressure the way attention does.

### 25. A mechanism that predicts the sign of a noise draw is not evidence

Earned 2026-08-08, by an agent who made this error **in the same session in
which it quoted the rule against it**, and caught it one rep later.

Rep 1 of an A/B showed +0.51 ms/field. A mechanism was found in the source that
explained it: gating a comparison stopped something from draining the write
barrier's dirty set, so `arm()`'s `dirty_len == 0` fast path failed and every
boundary bought a real `mprotect`. The prediction from the code was ~0.33
ms/field against +0.51 observed — **same order, derived from source, not fitted
to the number.** It was reported as a mechanism-backed regression.

**Rep 2 came back −0.22. The true result was +0.145 ± 0.365, both signs, a
null.** The mechanism explained a coin flip.

**The mechanism made the error worse, not better.** This is the part worth
internalizing. An unexplained one-rep delta feels like an unfinished
measurement, and unfinished measurements get a second rep. A one-rep delta with
a coherent, source-derived, quantitatively-close story attached feels like a
*finding* — and findings get written up. **Feeling explained is precisely what
stops you running rep 2.**

Note what was *not* wrong with the mechanism: it is real in the source,
`dirty_spans()` genuinely is consuming, and the fast path genuinely does depend
on something draining the set. **A correct mechanism can still be attached to a
noise sample.** Its correctness says nothing about whether it accounts for the
delta in front of you.

**The rule, stronger than "two reps minimum" because it names what defeats
that rule:** when a single rep produces a delta *and* you can explain it, treat
the explanation as a reason to be **more** suspicious, not less. Run the second
rep before writing the story down. If the effect is real the mechanism will
still be there in twenty minutes; if it is not, the story would have cost a
retraction — and, in this instance, a false finding relayed onward to the
project owner before it could be withdrawn.

**Corollary for reporting:** state the rep count and the sign pattern in the
same breath as the delta, always. "+0.51 ms" and "+0.51 ms, n=1" are different
claims, and only the second one is honest about what it is.

### 24. Ask what CONSUMES a call's side effects, not just what it computes

Earned 2026-08-08, by a fix that was **proven sound and bought nothing**. The
soundness proof was correct; it simply did not license the conclusion drawn
from it.

**Read the measurement note at the end of this rule before citing it.** The
coupling below is a *source* fact and it is real. The regression that was first
reported as its evidence **did not survive a second rep** and has been
withdrawn.

The scheduler-mirror reconcile looked redundant with the dispatch-loop
reconcile: same region, same baseline, same per-step rate. It was gated behind
`FN64_FAST_MUTATION_JOURNAL`, with a source proof that the thing being skipped
(`reconcile_snapshot_before_dispatch`) is a **pure detector** that mutates
nothing. The proof holds. The program **did not get faster** — the change
measured **+0.145 ms/field, sd 0.365, signs spanning both directions**, i.e.
nothing.

**The comparison had a second consumer nobody had accounted for.**
`matches_view` reaches `barrier_spans()` → `dirty_spans()`
(`write_barrier.rs:1230`), which is documented **CONSUMING**: it calls
`disarm_and_capture()` and *takes* the pending dirty set. That is precisely
what leaves `dirty_len == 0`, which is the condition `arm()` (`:808`) tests to
skip re-issuing `mprotect(PROT_READ)` over an already-protected span. Gate the
comparison, nothing drains the set, the fast path fails, and **every mirror
boundary buys a real ~1.2 µs syscall**: ~0.33 ms/field predicted from the code.

**That prediction was never confirmed, and must not be quoted as though it
were.** It matched rep 1's +0.51, which is how it came to be believed; rep 2
returned −0.22 and the two-rep result is **+0.145 ± 0.365, both signs — a
null**. So the measurement's actual verdict on this coupling is an **upper
bound**: whatever the drained-set effect costs, it is smaller than this
experiment can resolve, i.e. **under roughly 0.4 ms/field**. That bound is a
real and falsifiable result. The regression is not. See rule 25.

**The two questions that are not the same question:**

| question | answer here | catches this bug? |
|---|---|---|
| Does this call write state the program later reads? | No — pure detector | **No** |
| Does anything downstream depend on this call having RUN? | **Yes — the barrier's dirty-set drain** | **Yes** |

A pure-detector proof answers the first. Only the second finds a side-effecting
**producer** masquerading as a redundant check. **Two calls touching the same
region are not necessarily doing the same job — one may be feeding the other.**

**So: safe and worthless are compatible.** A soundness proof licenses a change;
it never predicts a benefit. Keep such proofs — they are correct and reusable —
but do not let one stand in for a measurement.

The tell was in this document the whole time and was misread: *"the ungated
scheduler-mirror reconcile arms the barrier one step ahead of the gated call,
leaving it an empty dirty set to compare."* That sentence describes the mirror
**doing work for the barrier**. It was read as evidence of redundancy; it is
evidence of **coupling**.

### 23. "Skippable?" is about whether a check WRITES, not what it looks like

Earned 2026-08-08 gating the scheduler-mirror reconcile, and corroborated by a
corruption investigation that reached the same boundary from the opposite
direction years of context apart.

Two functions in `live_program.rs` run what looks like the same check — compare
the watched executable region against its baseline, complain if it changed.
They are visually near-identical and differ in one call:

| | does it write? | gateable? |
|---|---|---|
| `reconcile_snapshot_before_dispatch` | **No.** Takes `&mut self`, mutates nothing; three O(1) asserts and a panic. The snapshot is dropped. | **Yes** |
| the host-ABI flush path (`:2810`) | **Yes.** Calls `adopt_snapshot`, which accepts current bytes as the new `expected`. | **Never** |

Skipping the second one leaves the baseline stale, and a later dispatch
re-detects a change that was already accepted. That is not hypothetical: it
produced `unjournaled executable mutation changed physical RDRAM
[0x0009b0b3, 0x0009b0b4)` at 3M steps, and the comment at
`live_program.rs:2670-2685` was written by whoever debugged it — concluding, in
its own words, that *"the reconcile check in `reconcile_before_dispatch` IS
skippable, because it only compares and never advances anything. That one stays
gated; this one does not."*

**The rule:** before gating any verification, ask *does this code path change
state that a later path depends on?* — not *does this look like a redundant
check?* A pure detector panics or does nothing observable, so removing it
removes a check. A detector that advances a baseline as a side effect is a
state machine wearing a check's name, and removing it removes a transition.

**Why this one is trustworthy.** A safety boundary discovered independently by
a **failure** (someone holding a live corruption) and by an **audit** (someone
hunting 8.43 ms), landing on the same side, is a real boundary rather than
either party's convenience. That agreement is worth more than either finding
alone, and more than the milliseconds that motivated the second one.

**Corollary on method:** this question is *decidable by reading the function
body* — it covers every route and takes minutes. Reach for a census when the
question is "how often does this state occur, and at what cost", which source
cannot answer. Soundness by reading, sizing by measurement; do not substitute
one for the other.

**What this rule does NOT license, learned the hard way the same day.** This
rule establishes that gating a pure detector is *safe*. It says nothing about
whether doing so is *profitable* — and in the case that produced it, gating the
mirror's detector bought **nothing at all**: +0.145 ms/field, sd 0.365, signs
spanning both directions over two interleaved reps. The change is sound,
correct, and pointless, and it was reverted on those grounds.

**Safe and worthless are compatible.** A soundness proof licenses a change; it
never predicts a benefit. See rule 24 (what the comparison was also doing) and
rule 25 (why one rep and a good story nearly made this a "regression" instead
of a null).

### 21. The counter's UNIT is part of the counter, and a rate inherits its error

Earned 2026-08-08 on the WM2000 choppy-audio investigation, which ran for hours
against a headline figure that was **wrong by exactly 2x**.

The claim was *"audio delivered at 91.5% of real time"* — 1,290,576 `samples`
over 44.1 s = 29,273/s against a 32,000 Hz guest clock. Every step of that
arithmetic is right except the unit. **`AudioOutputStats::samples` counts i16
CHANNEL samples, not frames.** `deliver_ai_buffer`
(`crates/fn64-abi/src/task_dispatch/setup.rs:160-171`) pushes one `i16` per
**2 bytes** across the DMA range and `:186` adds that length; the very same
function writes metadata at `:226-228` reading `channels=2` / `frames={len/2}`.
Stereo frames are `samples / 2`, so the real delivery is **14,632 frames/s =
45.7% of real time**, not 91.5%.

The two readings are not a rounding apart — they are different diagnoses. 91.5%
is an 8.5% shortfall that sounds like jitter or a scheduling boundary; 45.7% is
the emulator running at less than half speed. **Hours were spent looking for a
defect sized to the wrong number.**

The rule: **before dividing a count by wall-clock seconds, find the line that
increments it and read what one unit IS.** A rate is a quotient, and it
inherits the error of its numerator silently — nothing about "29,273
samples/sec" looks wrong. Where a counter's name is ambiguous across a known
axis (samples/frames, bytes/words, fields/frames, packets/messages), the name
alone is not evidence; the increment site is.

Corollary, since this codebase demonstrably has the ambiguity live: **finding a
units bug in one place is a reason to check its twin on every path that
consumes the same quantity.** Here the resampler was the obvious suspect and
was cleared — but by injecting a 2x error into
`BandlimitedResampler::process`'s `step` and confirming **3 of 6 tests failed**,
not by reading it. That is rule 6a: a units check that cannot fail is not a
check.

### 22. A healthy delivery path is not a healthy stream

The same investigation, and the reason the units error survived so long.

`backend_buffers == ai_buffers` held **exactly** throughout, i.e. cpal accepted
every buffer and dropped none. That was read as *audio is healthy*. It is not
that claim. **Equal counts prove nothing is being LOST; they say nothing about
whether enough is ARRIVING.** The device was playing gaps because the producer
was starving it, and a zero-drop counter reports that state as perfect.

The general shape: for any producer/consumer seam, *delivered ÷ wall-clock
seconds against the rate the consumer demands* is the health metric. Queue
depths, drop counts, and buffer parity are all **downstream** of it and every
one of them reads clean under starvation.

**The contradicting instrument existed and was not on the line.**
`AudioStreamHealth` already carried `underrun_samples` — the count of samples
the callback zero-filled, which is starvation measured directly — and
`crates/fn64-shell/src/main.rs:671-707` prints it. But
`examples/wm2000-block-boot/src/shell.rs`, the binary actually being played,
read that struct and printed those fields **only on SPIKE lines**, never in the
routine heartbeat. So a starved run and a healthy run emitted the same quiet
heartbeat, and the one number that could have falsified the diagnosis was
absent from the only output anybody was reading. Fixed: the heartbeat now
prints `underrun_samples`, `late_callbacks`, `max_callback_gap_us`, and a
computed **delivered-frames-per-wall-second against the guest's programmed
rate**.

This is rule 7's family (*a line printed on a state CHANGE cannot prove absence
of progress*) pointed at a diagnostic rather than a route: **put the counter
that can contradict you on the routine line, not the exceptional one.** A
statistic that only appears when something already looks wrong cannot tell you
that something is wrong.

### 19. A size difference proves "something changed", never "the thing you meant"

I asserted two A/B binaries were the right lanes because one was **4.4 MB
smaller** (95,304,208 vs 90,878,784, distinct SHAs) — *"exactly the shape of
removing a per-instruction verification block from every runner."* I called it a
stronger lane check than a source grep, and I was **wrong**.

Reading the *generated source* the armed build actually wrote showed the
detector still present — `EXPECTED_WORDS` tables and `verify_live_word!` calls
in the emitted `'run: loop`, 64 references in one runner alone, with a verified
build-time mtime. **Had the A/B run, it would have measured verify-on against
verify-on and reported a fabricated delta** — the 4.9x incident's exact shape.

The size *had* changed, because 32 of 142 runners regenerated. A partially
regenerated catalog is a different binary and an invalid experiment, and a byte
count cannot tell those apart from the change you intended.

**Root cause worth knowing on its own: `cargo:rerun-if-env-changed` only takes
effect from the run that emits it onward.** On the first build after adding a
new env-gated flag, cargo has no recorded dependency on that variable, so the
build scripts do not re-run and the flag silently does nothing. Force it with
`touch <build.rs>` on the first build after introducing the gate.

The general rule: **verify the state you meant to cause, at the layer that owns
it.** For a codegen flag that layer is the emitted source, not the binary's
size. This is rule 15 again, and note it bit the *reviewer* — I proposed the
weaker check to an agent that was already doing the stronger one.

### 18. `pcpu` and `pgrep -f` both lie about this workload
Two liveness checks that read plausibly and are wrong, both rule-15 shapes,
both caught in practice on this project:

- **`pcpu` reads 0.0 while the benchmark is running at 100%.** A healthy run
  was nearly declared hung on that basis. **CPU TIME advancing is the only
  honest liveness check** — sample it twice and compare.
- **`pgrep -f wm2000-block-boot` counts your own monitoring shells**, and any
  `eval`/snapshot wrapper whose command line contains the string. It reported
  **2 concurrent benchmarks when there was 1**. Match on args and exclude the
  wrappers, or count by CPU.

A third instance of the same family: sampling a **recycled PID** that now
belongs to an unrelated process. A PID is not a durable handle; re-verify the
identity, not just the number.

**Two more on 2026-08-08, from OPPOSITE directions, within minutes of each
other. The generalization is the point: a query that cannot distinguish
"absent" from "never looked" reports both as zero.**

- **`comm` attributed 17 rustc to the wrong owner.** An agent holding a queued
  build read `ps -eo pcpu,comm`, saw 17 busy rustc, and reported "the peer's
  shard rebuild is running" — twice, in status messages. The peer's lock had
  already released and **every one of those processes was the reporting
  agent's own**, compiling shards into its own `target/`. `comm` shows the
  binary and drops the argument list, which is where the identity lives:
  `ps -eo args` named the target directory immediately. Note the failure is
  *stable* — "is rustc busy" would have answered "peer is building" forever,
  because the check cannot represent ownership at all.
- **A glob that matched no files reported "0 references."** A grep for
  `EXPECTED_WORDS` whose file glob matched nothing returned zero hits, which
  reads exactly like *the detector has been deleted* — the alarming answer,
  indistinguishable from *the question was never asked*. Caught only because
  the count was suspiciously clean.
- **`clang` matched inside `installd`, and the contention detector cried
  wolf.** A rule-18-compliant detector — sampling advancing CPU *time*, not
  `pcpu` — reported CONTENTION mid-benchmark on two pids. They were
  `system_installd` and `installd`, macOS package daemons that had matched an
  unanchored `clang` pattern **as a substring of their own names**, burned
  0.03 s between them, and exited. No compiler was running at any point.

  This is the mirror image of the `pgrep -f` entry above: that one **matched
  itself**, this one **matched something unrelated**. Same root cause — a
  process query that cannot distinguish the thing it means from a string that
  merely contains it.

  **The remedy generalizes: match on the anchored BASENAME, never a substring
  of the full command.** Strip the directory (`sub(/.*\//,"",n)`) and compare
  for equality against an explicit set (`rustc`, `cargo`, `clang`, `clang++`,
  `ld`, `lld`, `cc1`, `rustdoc`) rather than testing whether a pattern appears
  anywhere in the line. A substring match over process names is never correct,
  because process names are not a namespace anyone controls.

  Note the failure was *safe but expensive*: a false CONTENTION costs a
  discarded pair and a re-run, where a false ALL-CLEAR costs a fabricated
  result. Bias detectors toward false positives, then anchor them so they stop
  firing.

**The fix for both is rule 6a's ten-second test**: run the query against the
state it is supposed to reject and confirm the answer *changes*. A grep that
returns zero against a tree where the symbol certainly exists is broken, and
that takes one command to establish.

**What is NOT the fix: substituting a byte-size delta.** An earlier revision of
this entry credited the same session's size check — verify-on 95,304,208 vs
verify-off 90,878,784, a 4.4 MB difference — as the one that worked, on the
reasoning that its failure mode is a wrong number rather than a zero. **That
was wrong and the credit is withdrawn. The size check did not work; it nearly
fabricated the A/B.** Only 32 of 142 runners had regenerated, the detector was
still present in the emitted source of the "off" lane, and trusting the delta
would have measured verify-on against verify-on. See rule 19: **a size
difference proves something changed, never which thing.**

Two things worth keeping from how that error was made. It bit the **reviewer**,
who proposed the weaker check to an agent already doing the stronger one and
described it as stronger. And the revision that credited it was written by an
agent that had *already read* rule 19's entry two paragraphs above, noticed the
tension, and reconciled it with an invented "narrower rule" instead of asking
whether the premise was true. **A contradiction with an adjacent committed entry
is evidence you are wrong, not an invitation to harmonize.**

### 17. Do not budget instrumentation cost by counting clock reads
An agent predicted its five new timers would cost **0.029 ms/field** — 33.8 ns
measured per `Instant` pair × 856 reads. Measured: **+1.62 ms/field, +4.6% on
the render field.** Wrong by **56x**, and it had told the coordinator the
instrument was safe on that basis.

**Arming a timer in the hottest loop costs what it does to inlining, register
pressure and branch layout — not what the clock read costs.** The clock is the
cheap part.

Consequences to apply, not just to note:
- Always run an **armed/control pair** and correct absolutes by the ratio.
  Shares survive; absolute ms do not.
- Any phase measuring **below the perturbation** is below the instrument's
  resolution. Here the two sub-0.1 ms guard rows are unresolvable, and saying
  "0.037 ms" about them overstates what was learned.
- A budget of this shape is a *lower bound on the error*, never an estimate.

### RETRACTED: the "guest code is only 1.01 ms/field" figure was fabricated

I claimed a layer partition measured guest code at **1.01 ms/field (4.5%)**,
quoted it repeatedly, contrasted it with the 21.72 ms executor self, called the
result a **21x contradiction**, and dispatched an agent to resolve it.

**Neither "1.01 ms" nor "4.5%" appears anywhere in this repository.** I checked.
The number does not exist.

The real figure is **2.86%**, in
`docs/plans/resolvable-self-time-profile.md:111` — a **self-time share**, not a
per-field duration, and measured on the **19,523-step route that renders
nothing** (`gfx_submits=0`, eight VI interrupts; see rule 11). Reconstructing:
420 ms total, 84.5% steady = 354.9 ms, of which 2.86% = **10.15 ms of guest
code for the entire run**, over ~8 fields = 1.27 ms/field. That is almost
certainly what I rounded to "1.01".

**There was never a contradiction to explain — there was a category error.**
2.86% is a self-time share on a non-rendering route running 2,440 steps per
field; 21.72 ms is a wall-time total on a rendering route running 185.5 steps
per field carrying 2.88 display lists. The two quantities were never about the
same thing, and no per-field figure transfers between those routes.

This is rule 11 and rule 2 compounding: a share read as a duration, taken from
a route with no frames in it, and carried into a table about a route that has
them. **Before quoting any per-field figure, state which route produced it and
whether that route rendered anything.**

### The watched region is reconciled ~3.6x per step, and only one site is gated

Verified by reading, not inferred. Two independent reconciles of the **same
region against the same baseline** run per step, microseconds apart, with
nothing writing guest RDRAM in between:

| site | path | gated by `FN64_FAST_MUTATION_JOURNAL`? |
|---|---|---|
| **mirror** | `host.rs:208` → `execution.rs:772` → `:808` → `reconcile_before_dispatch_from_view` (`live_program.rs:2205-2225`) | **NO** |
| **dispatch** | `runners.rs:1029/1033` → `reconcile_before_dispatch` (`:2146`) | yes |

`host.rs:367-371` states the mirror's true cost in its own words: *"under
`recomp-rs` this is a FULL watched-region journal reconcile (see
`EXEC_MIRROR_NS`), not the four-byte store its name suggests, and **it runs on
every step**."* And `reconcile_before_dispatch_from_view` has **no
`continuous_snapshot_enabled()` check** — it calls
`reconcile_matched_before_dispatch` and `reconcile_snapshot_before_dispatch`
unconditionally, including all three O(1) asserts.

Measured: **274.9 steps and 991.5 `barrier_served` boundaries per render field
= 3.61 comparisons per step.**

**This closes the correctness question, in the flag's favour.** Both earlier
arguments were wrong:

- Mine ("no second detector, therefore load-bearing") — there *is* a second
  detector, and it is the ungated one.
- The counter-argument ("the repository records this flag failing") — that
  incident belongs to the reverted write-queue gate and the baseline-*advancing*
  read, not to this call site.

With the flag on, an undeclared write is still caught **at the very next step's
mirror**, by the same code with the same diagnostics and the same asserts.
Detection is delayed by at most one step, not lost. The gated site is a
**second** comparison, not the last line of defence. The C-shim escape path
(`live_program.rs:2088-2094`, mitigation with zero non-test callers) remains
real and worth wiring, but it is **orthogonal** — equally uncaught-until-next-
boundary with the flag off.

**It also supplies a benign explanation for the two historical "measures zero"
entries**: if the mirror just proved the region clean microseconds earlier, the
dispatch comparison takes its cheap early-out and is already nearly free. That
requires no reachability bug. Pre-registered as the leading hypothesis before
the A/B lands.

Whichever site pays, **the redundancy itself is the finding** — and the mirror,
at a measured 8.43 ms/field, is the ungated one.

### The dispatch comparison is already 364x optimized — it cannot be the remainder

Before dispatching work at `reconcile_before_dispatch` (the named lead into the
11.96 ms remainder), the existing counters answer how expensive it can possibly
be. **It is already near-optimal**, and the arithmetic says so:

Per render field, from the population split:

| | |
|---|---|
| `barrier_served` | **991.5** |
| `barrier_fell_back` | **0.0** — the barrier answered *every* time, zero full scans |
| clean boundaries | 745.6 (**75%**) — compare **zero bytes** |
| dirty boundaries | 245.9, comparing **697 pages** = 2,790 KiB |
| if it always scanned 1 MiB | 992 MiB |
| **speedup already realized** | **~364x** |

`matches_view` (`live_program.rs:359-380`) takes `barrier_spans()` and clips the
comparison to dirty pages only, falling back to a full scan **only** when the
barrier cannot answer — which measured **never** on this route.

**So the 1 MiB-per-dispatch framing is wrong for the current code.** The
comparison touches ~2.7 MiB per render field, not ~992 MiB, and three quarters
of its boundaries do no comparison at all. Whatever the 11.96 ms remainder is,
**this is not the bulk of it** — and my earlier "prime suspect" reasoning, which
priced a 1 MiB reconcile per boundary, was pricing code that does not run.

Two smaller notes from the same read:

- The `TEMPORARY (mprotect feasibility census, 2026-08-07)` call at `:360-363`
  inside the hottest comparison **is properly gated** (`note_boundary`
  early-returns unless enabled, `snapshots.rs:1018-1021`). Not a cost. It is
  still worth deleting once the census is finished, since "TEMPORARY" in the
  hot path invites exactly the suspicion it just cost to dispel.
- `barrier_spans` documents the only failure mode honestly: `None` means "the
  barrier cannot answer" and every caller runs the full scan; **it never returns
  a span list that omits a page it saw written.**

### MEASURED: the executor split — apparatus is 24%, not the majority

`f4aeb1c`, five sub-counters inside `run_one_step` behind `FN64_EXECUTOR_SPLIT`,
reported by population with **nested counters subtracted, not summed**. Render
field, 37.09 armed / 35.47 control:

| phase | ms | share | kind |
|---|---:|---:|---|
| `gfx_ns` | 12.53 | 35.3% | graphics (rdp 7.04, rsp 5.98) |
| **remainder inside the resume** | **11.96** | **33.7%** | guest + runtime |
| mirror boundary | **8.43** | 23.8% | apparatus, **per-step** |
| audio_lle / vi_present | 1.14 / 1.14 | 3.2% each | |
| guard @ suspend / device | 0.037 / 0.038 | 0.1% | apparatus |

**The hypothesis that executor self is mostly apparatus is REFUTED.** Named
apparatus totals **24%** of the field, and the mutation journal's two flush
seams come to **0.075 ms combined** — below this instrument's own perturbation.
Making the guard cheaper *at those seams* is a closed line.

**The 14.41 ms mirror prediction was 1.7x high** (measured 8.43), for exactly
the reason its own falsifier named: a cheap early-out when nothing is dirty.
Pre-registering the estimate is what made that judgeable.

Three redirections:

1. **A new 11.96 ms remainder appeared inside the coroutine resume** — guest
   code plus the runtime it calls synchronously: `RdramView` reads,
   `translate`/`backing_offset`, per-instruction validation, and
   `runners.rs:1033`'s `reconcile_before_dispatch`, **which runs per loop
   iteration, not per step**. This is now the largest unnamed cost.
2. **The mirror is per-STEP, not per-submit** — and *cheaper* per call on render
   fields (32.06 vs 59.11 µs), 2.17x total against a 4.01x call ratio. Its
   bimodality is entirely "render fields take 4x more steps".
3. **Steps per field (274.9 vs 68.5) is therefore a lever nobody has examined**,
   and it multiplies everything per-step. The 79 µs/call resolves as many cheap
   operations — 130.5 µs/step inclusive, no single phase dominating.

**RDP drift, flagged not absorbed:** `gfx_lle_rdp` is 7.04 ms/render-field
against the 5.82 baseline (+20.9%) while `gfx_lle_rsp` is flat (−1.7%). Same
asymmetric signature as the earlier unexplained +8%, about half the magnitude,
and **present in the control lane** — so it is not the instrument. Still
unexplained, and it is on the line we are about to work.

### Prime suspect for the 21.72 ms: a 4-byte mirror that reconciles 1 MiB (measured 8.43 — see above)

**UNCONFIRMED — sized before the measurement, so the result is judged against a
prediction.** Found by an explorer reading the dispatch prologue, not by a
profiler; no existing counter sees it.

`mirror_guest_running_thread` (`host.rs:188`) sounds like a 4-byte store, and
its own range is exactly 4 bytes (`execution.rs:785`). Under `recomp-rs` it
delegates to `commit_scheduler_running_thread_mirror` (`execution.rs:772`),
whose comment states the cost outright:

> *"Scheduler selection is a dispatch boundary, so this reconciles the whole
> watched region — 1 MiB on WM2000 — every time a thread is picked."*

It runs **in the prologue of every step, outside the coroutine resume** — so it
is inside `executor_ns` and invisible to every phase counter.

At 274.9 executor calls per render field:

| | |
|---|---|
| reconciled per render field | **0.29 GB** |
| over the 5,660 render fields | **1.6 TB** |
| cost at ~20 GB/s (word-wise memcmp) | **14.41 ms/field** |
| cost at ~2 GB/s (byte closure) | 144 ms/field — impossible, so the fast path must engage |

**14.41 ms would be 66% of the 21.72 ms executor self.** The byte-closure figure
exceeding the whole field is itself informative: it bounds which path is live.

If this holds it is the answer to "why isn't modern hardware enough" — the
render field is not dominated by emulating an N64, it is dominated by a
correctness reconcile that runs once per scheduler selection and scales with
how much code the guest executes. **Render fields make 4.0x more scheduler
selections than off-fields (274.9 vs 68.5), which is exactly why the cost is
bimodal.**

Falsifiers: the reconcile may already take a cheap early-out when nothing is
dirty (75% of barrier boundaries find nothing); the view path copies word-wise
and may be far faster than 20 GB/s assumed here; and the 4.0x call ratio may
not translate to a 4.0x cost ratio if the dirty set differs by population.

### MEASURED: the 21.72 ms splits 39% apparatus / 55% guest+runtime. The mirror is 8.43 ms.

Measured 2026-08-08 at `5f21996` + this instrument, route `a9e1b25e`, RT64,
headless, 2.1M steps, quiet machine (load 2.98). Gate:
`FN64_EXECUTOR_SPLIT=1`, separate from `FN64_PHASE_TIMING` so its perturbation
is itself measurable. Guest byte-identical in both lanes — `gfx_submits=16586`,
`audio_submits=11005`, `sp_tasks=27591`, `vi_interrupts=12008`,
`controller_ops=3115`, `sim_time=18776001537`, `render_error=None`.

**The render field, fully named for the first time.** Values corrected for the
instrument's +4.6% perturbation (see below); shares are perturbation-robust.

| phase | ms/field | share | kind |
|---|---:|---:|---|
| ~~**mirror boundary**~~ (`mirror_guest_running_thread`) | ~~**8.43**~~ | ~~**23.8%**~~ | apparatus |
| `gfx_ns` (nested in the resume) | 12.53 | 35.3% | graphics |
| — `gfx_lle_rdp_ns` | 7.04 | 19.0% | |
| — `gfx_lle_rsp_ns` | 5.98 | 16.1% | |
| **remainder inside the resume** | **11.96** | **33.7%** | see below |
| `audio_lle_ns` | 1.14 | 3.2% | |
| `vi_present_ns` | 1.14 | 3.2% | |
| guard @ suspend | 0.037 | 0.1% | apparatus |
| guard @ device | 0.038 | 0.1% | apparatus |
| `run_one_step` residual | 0.014 | 0.0% | |

> **CORRECTED 2026-08-08 — the mirror row is struck because 23.8% is wrong for
> the population that matters, and the error is instructive.** Re-measured with
> all three gates armed on the full route: the mirror is **9.13 ms = 16.2% of a
> 56.23 ms slow field**, and the *whole* apparatus is **16.4%**. The 23.8% here
> was computed against a smaller field total and read as though it applied to
> the render population.
>
> **The deeper error is the one to learn from: this row was sized over the
> wrong population.** The mirror is **57.0% of a fast field** and **16.2% of a
> slow one**. A share that large on the fast row is what made it look like the
> biggest line in the program, and fast fields are the ones that already fit in
> budget. See "THE RENDER FIELD IS 83% GUEST+RUNTIME" above; do not size a
> candidate from this table without checking which population it pays into.

**The mirror prediction was 14.41 ms; measured is 8.43 — the estimate was 1.7x
high, and the falsifier it named is the reason.** The doc above guessed a
20 GB/s word-wise memcmp over 0.29 GB/render-field. The real path takes the
cheap early-out often enough to land at **32.06 µs per call** on render fields.
Note it is *cheaper per call* on render fields than on off-fields (32.06 vs
59.11 µs): it is a **per-step** cost, and render fields simply make 4.0x more
steps. Its 2.17x population ratio against a 4.01x call ratio says so directly.

**Two conclusions, and the second is the one that redirects work.**

1. **The named apparatus is 8.51 ms — 24% of the field, not the majority.**
   The brief's hypothesis was that executor self is "mostly apparatus". It is
   not: the mutation journal's two per-yield/per-device seams measure
   **0.075 ms combined**, essentially nothing, and the whole named guard is
   under a quarter of the field. `FN64_FAST_MUTATION_JOURNAL`'s recorded zero
   was not a reachability accident here — those seams really are cheap now.
2. **A NEW 11.96 ms remainder appeared, and it is inside the coroutine
   resume.** `resume NET` (resume minus mirror minus guard) is 26.80 ms, of
   which `gfx_ns` accounts for 13.10 and audio 1.19. The 11.96 ms left over is
   **translated guest code plus the runtime it calls synchronously** — RDRAM
   reads/writes through `RdramView`, `translate`/`backing_offset`, the
   per-instruction validation, and the dispatch loop's own
   `reconcile_before_dispatch` at `runners.rs:1033`, which runs **once per
   step**.

   ~~which the explorer found runs **once per loop iteration**, not once per
   step.~~ **CORRECTED 2026-08-08 in place, because the original phrasing sent
   a later reader hunting for a loop-hoist win that does not exist.** One loop
   iteration IS one step: `run_catalog_block_program`'s loop body ends in
   `crate::suspend_active_coroutine(...)` (`runners.rs:1113`), which suspends
   the coroutine, so the loop does not spin within a step — it resumes where it
   suspended. "Per loop iteration" and "per step" are the same rate here. See
   "SIZED BY READING, NOT TAKEN" below.

**So "executor self" was two different things in one bucket:** a per-step
apparatus boundary (the mirror, 8.43) and a per-step guest+runtime cost
(11.96). At **45.47 µs per step** on render fields against 25.70 on off-fields,
the remainder is mostly per-step with a per-submit component (7.09x total ratio
against a 4.01x call ratio).

**The 79 µs/call headline resolves as "many cheap operations".** A render-field
step costs 130.5 µs inclusive; no single phase dominates it. The step count
itself — 274.9 per render field against 68.5 — is the multiplier on everything,
which makes *steps per field* a lever nobody has examined.

**Instrument perturbation, and my own budget arithmetic was wrong.** Mean
23.08 armed against 22.18 control, **+4.1%**; render field 37.09 vs 35.47,
**+4.6%**. I predicted 0.029 ms/field from 33.8 ns × 856 clock reads and
measured **1.62 ms — 56x that**. `Instant::now` is not the cost; the cost is
what arming it does to the surrounding code (inlining, register pressure,
branch layout in the hottest loop in the program). **Do not budget
instrumentation by multiplying a microbenchmarked clock read by a call count.**
The control at 22.18 sits just below the 22.36–22.41 band; the armed lane at
23.08 is outside it, so the absolute ms above are corrected by the
control/armed ratio and the *shares* are the robust figures. The two
sub-0.1 ms guard rows are smaller than the perturbation and should be read as
"below this instrument's resolution", not as precise values.

**Flagging the RDP line as required, not absorbing it.** `gfx_lle_rdp_ns` is
**7.04 ms/render-field against the 5.82 baseline, +20.9%**, while
`gfx_lle_rsp_ns` is 5.98 against 6.08, **−1.7%**. That is the same asymmetric
signature as the unexplained +8% drift recorded above (RDP up, RSP flat or
down), at about half the magnitude, and it is present in the *control* lane
too, so it is not my instrument. Still unexplained.

**UPDATE 2026-08-08 — the drift is ABSENT in the `FN64_FAST_MUTATION_JOURNAL`
A/B, reported as a negative because a drift that comes and goes is a different
problem from one that persists.** Same route, same binary, same RT64 headless
lane, `FN64_PHASE_TIMING=1`: `gfx_lle_rdp_ns` reads **5.85 ms/render-field
against the 5.82 baseline (+0.5%)** and `gfx_lle_rsp_ns` **5.97 against 6.08
(−1.8%)**. Both lanes agree (B: 5.87 / 6.04). So the +20.9% excursion did not
reproduce, and whatever causes it is **intermittent between runs rather than a
standing property of the route or of phase timing**. That rules out the
simplest explanations — it is not the instrument, and it is not a permanent
regression introduced between the baseline and now.

### SIZED BY READING, NOT TAKEN: `runners.rs:1033` is per-STEP, not per-iteration

Recorded because a defect found and rejected should not need rediscovering.

The `executor_ns` split above named "`reconcile_before_dispatch` at
`runners.rs:1033`, which runs **once per loop iteration**, not once per step"
as a component of the 11.96 ms resume remainder. Read as "many reconciles per
step", that would be the cheapest and safest target available — no redundancy
argument required, just a hoist out of a loop.

**It is not that.** `run_catalog_block_program`'s loop body ends in
`crate::suspend_active_coroutine(...)` at `runners.rs:1113`, inside the
`dispatched.instructions > 0` arm. That call suspends the coroutine, so **one
loop iteration is one scheduling step** — the loop does not spin within a step,
it resumes where it suspended. "Per loop iteration" and "per step" are the same
rate here, and the phrasing in the split section is misleading rather than
wrong.

The reconcile sites and their true rates:

| site | rate |
|---|---|
| `runners.rs:1029` (pre-loop) | once per `run_catalog_block_program` entry |
| `runners.rs:1033` (in-loop) | **once per step** |
| `execution.rs:808` (the mirror) | **once per step**, ungated |
| `runners.rs:639`, `:870` | dynamic-mapped lane, not this route |

So there is **no loop-hoist win**, and the two per-step sites are the mirror
and `:1033` — which is exactly the redundant pair the doc already identifies,
at the same rate, not a separate cheaper defect. **Prefer-the-safer-target
reasoning does not apply, because the safer target does not exist.**

The one genuine cheap fix in this area remains the separable defect already
filed: `continuous_snapshot_enabled()` gates *above* three O(1) assertions
(`live_program.rs:680-692`), where only the memcmp was meant to be skippable.
That is a correctness-of-gating fix worth making on its own terms, but it is
O(1) work and cannot be a measurable share of 8.43 ms.

### WHICH ROUTE: 35.24 ms/field, and why the 22.51 figure does not apply here

Recorded because two figures from different routes were nearly combined into
one subtraction, which is rule 11 in its most expensive form.

**This fix is measured on `render-benchmark.zsh` at its default 1.5M steps**,
which baselines at **35.24 ms/field (2.11x budget)** — matching the 35.84 /
35.47 / 35.72 band every earlier mirror measurement used. That is the correct
route for this change, because **it is the route the 8.43 ms mirror figure was
measured on.**

**The 22.51 ms figure in "THE STANDING BAR" is a different route and must not
be subtracted from.** An arithmetic of `22.51 − 8.43 = 14.1` was proposed and
is void: the 8.43 was never measured on the route that produces 22.51. Two
numbers from two routes, one subtraction, an answer that means nothing — the
same error shape as the retracted "1.01 ms guest code" figure above.

**Always state the route beside a per-field figure.** The doc already says this
(rule 11); this is the second time it has been needed in one week.

**On the byte-identity tuple.** The gate values quoted in this session
(`gfx_submits=16586`, `audio_submits=11005`, `sp_tasks=27591`,
`vi_interrupts=12008`, `controller_ops=3115`, `sim_time=18776001537`) belong to
a **~2.1M-step** route. At 1.5M steps this route ends at `gfx_submits=11153`,
`audio_submits=7685`, `sp_tasks=18838`, `vi_interrupts=8386`,
`controller_ops=2390`, `sim_time=13112786076`, `render_error=None`.

**The gate was NOT redefined to match what the run produced.** That would make
every future comparison meaningless — a gate rewritten to fit its own result is
not a gate. Instead: the A/B's validity rests on lanes A and B being identical
routes *to each other*, which they are, and guest identity is verified
separately at the step count that produces the specified tuple. Determinism on
this route is independently evidenced by the documented submit checkpoints
(452 @200k, 1758 @400k, 3356 @600k) reproducing exactly, plus
`render_error=None`.

### PRE-REGISTERED, before measuring: only ONE of the two reconciles is gated

Written down before the A/B runs, so that a confirmation cannot become a
post-hoc story and a refutation cannot be quietly dropped (rule 1's discipline
applied to a mechanism rather than to a candidate).

**The claim, from source alone.** The two per-step reconciles of the same
region against the same baseline are gated differently:

| site | function | `continuous_snapshot_enabled()` check? |
|---|---|---|
| dispatch loop, `runners.rs:1033` | `reconcile_before_dispatch` (`live_program.rs:2146`) | **YES**, at `:2160` — returns right after `seal_with` |
| scheduler mirror, `execution.rs:808` | `reconcile_before_dispatch_from_view` (`:2204`) | **NO** — runs `matches_view` unconditionally |

**What it predicts.** `FN64_FAST_MUTATION_JOURNAL=1` can only switch off the
site that the barrier has already made nearly free, while the 8.43 ms ungated
mirror keeps running at full cost. So the flag should measure ~zero — which is
exactly what three interleaved pairs found (**−0.14 ms/render-field, sd 0.35**,
deltas spanning both signs).

**Why this is worth pre-registering rather than just asserting.** The doc
reached the same mechanism *from measurement* at line 585 ("the ungated
scheduler-mirror reconcile arms one step ahead of the gated call, leaving it an
empty dirty set to compare"). Source-reading and measurement arriving at the
same mechanism independently is the strongest evidence available here, and it
is only strong if the source claim is recorded before the confirming
measurement is run rather than after.

**The falsifier, stated in advance.** If gating the mirror does *not* move the
render field by an amount consistent with its measured 8.43 ms, then the mirror
is not paying what the split attributes to it — most likely because the barrier
early-out already makes most of those 274.9 calls nearly free and the 8.43 ms
is concentrated in a few dirty boundaries. In that case the finding is that
**the mirror is cheap per call and expensive only in aggregate**, and deletion
of the comparison is not the lever; reducing steps per field is.

**What must be proven before any removal, and it is not structural similarity.**
Two checks that look alike are not redundant unless they are redundant *under
the states that actually occur* (rule 6a's converse). The redundancy argument
therefore requires, per population:

- how often `matches_view` at the mirror finds the region clean vs dirty, and
- **the dirty-page COUNT when dirty, not just the boolean.** `matches_view`
  clips to MMU-reported dirty pages and that clipping is a measured ~364x
  optimization, so "dirty" spans a wide range of real work: one dirty page and
  400 dirty pages are both "dirty" and cost very differently. A clean/dirty
  ratio alone cannot size what removal would save.
- whether the gated site would have caught anything the mirror did not, on the
  render-field population specifically.

### THE RENDER FIELD IS 83% GUEST+RUNTIME. The whole guard apparatus is 16.4%.

Measured 2026-08-08 on the full 1.5M-step `render-benchmark.zsh` route, RT64
headless, quiet machine, with all three required gates armed
(`FN64_PHASE_TIMING=1 FN64_EXECUTOR_SPLIT=1 FN64_FRAME_CENSUS_POPULATIONS=1` —
see rule 27). **This supersedes the apparatus framing in the split table
below.**

**The slow (render) field, the population that fails the bar:**

| row | ms/field | % of executor_ns |
|---|---:|---:|
| `executor_ns` (278.9 calls, 201.7 µs/call) | **56.232** | 100% |
| — resume (Executor step) | 55.966 | 99.5% |
| — — *(of)* **mirror boundary** | **9.129** | **16.2%** |
| — — *(of)* guard @ suspend | 0.039 | 0.1% |
| — — *(of)* **resume NET** | **46.798** | **83.2%** |
| — devtime (advance) | 0.251 | 0.4% |
| — — *(of)* guard @ device | 0.046 | 0.1% |
| — residual | 0.015 | 0.0% |
| **APPARATUS** (mirror + both guards) | **9.214** | **16.4%** |
| **GUEST+RUNTIME** (resume net) | **46.798** | **83.2%** |

**Delete the entire named apparatus and the render field is still 46.8 ms —
2.8x the 16.667 ms budget.** Every guard-side optimization available, taken
together and taken to zero, does not approach the bar. That closes the guard as
the primary target on this route.

**The bimodality inverts the mirror's apparent importance**, and this is the
part that explains every null measured against it:

| | fast field | slow field |
|---|---:|---:|
| `executor_ns` | 7.30 ms | 56.23 ms |
| mirror | 4.16 ms (**57.0%**) | 9.13 ms (**16.2%**) |
| resume NET | 3.01 ms (41.2%) | 46.80 ms (**83.2%**) |
| mirror µs/call | 59.36 | 32.74 |

**The mirror dominates the population that already has headroom and is minor on
the one that does not.** The census says it in its own output: *"A saving in a
row that is large on the fast row and small on the slow one pays into the
population that already has headroom."* Optimizing it buys time on fields that
already fit — which is precisely why the A/B measured a null.

Note the mirror is *cheaper per call* on slow fields (32.74 vs 59.36 µs); its
larger absolute cost there is entirely the 4.0x call count. It is a **per-step**
cost, and slow fields simply take more steps.

**Consequences, and they redirect the whole effort:**

1. **"Removing the entire guard lands near 1.27x" is not supported on this
   route.** Removing the entire named apparatus lands at **2.8x**.
2. **The next lever is `resume NET` — translated guest code plus the runtime it
   calls synchronously.** It is **83% of the render field and has no
   sub-counters**, exactly the position `executor_ns` was in before it was
   split. The same rule 2 move, one level deeper, is the next measurement.
3. **Stop sizing candidates against the apparatus.** Anything inside the 16.4%
   is competing for a sixth of a field that is 2.8x over budget.

#### THE NEXT MEASUREMENT: split `resume NET`, which is 83.2% and unnamed

Stated as the named next step so whoever picks this up starts from *"83% is
unnamed"* rather than rediscovering it.

`resume NET` = **46.798 ms of a 56.23 ms render field** and has **no
sub-counters**. That is exactly the position `executor_ns` was in before
`f4aeb1c` split it — and splitting it is the same rule 2 move, one level
deeper. From what is already known it contains at least:

- **`gfx_ns`** (RSP microcode interpretation + the raw RDP seam), previously
  ~12.5 ms/field on this route
- **`audio_lle_ns`**, ~1.2 ms/field
- **translated guest code** — the recompiled blocks themselves
- **the runtime they call synchronously** — `RdramView` reads/writes,
  `translate`/`backing_offset`, per-instruction validation, and the dispatch
  loop's own `reconcile_before_dispatch` at `runners.rs:1033` (per-step; see
  "SIZED BY READING")

**Even after subtracting graphics and audio, roughly 33 ms/field has no name.**
That is 2x the entire frame budget, in one bucket, on the population that fails
the bar. **The 60fps bar is won or lost inside translated guest code and the
runtime it calls synchronously, and nothing in the apparatus can reach it.**

Arm it with all three gates (rule 27) and read the **slow** row (rule 28).

#### The harness fix earned itself back on the run it was written for

The per-run unfiltered log added during this session (rule 27) mattered
immediately: the filter dropped `[executor-split]` again on **this very run**,
and all 20 lines survived only in the unfiltered copy. Without it this would
have been a third wasted 50-minute pair. **Fix an instrument the moment it
bites, rather than working around it** — the payback here was under an hour.

### MEASURED, AND REVERTED: gating the mirror comparison buys NOTHING

The pre-registered falsifier fired. Recorded in full because a sound change
that is also pointless is exactly the result most likely to be re-derived.

Route `render-benchmark.zsh` default (1.5M steps), RT64 headless, quiet machine
(contention detector armed and clean throughout), interleaved **A B A B** with
disjoint ranges, two reps. Lane A = `FN64_FAST_MUTATION_JOURNAL` unset (mirror
comparison runs); lane B = `=1` (comparison gated).

| run | mean | p50 | p95 | p99 | fields |
|---|---:|---:|---:|---:|---:|
| rep1 A | 35.24 | 33.18 | 65.65 | 68.25 | 7699 |
| rep1 B | 35.75 | 33.83 | 65.86 | 69.54 | 7699 |
| rep2 A | 35.71 | 33.59 | 65.42 | 70.21 | 7699 |
| rep2 B | 35.49 | 33.38 | 66.40 | 69.65 | 7699 |

| rep | delta (mean) | delta (p99) |
|---|---:|---:|
| 1 | **+0.51** (+1.45%) | +1.29 |
| 2 | **−0.22** (−0.62%) | −0.56 |
| **mean** | **+0.145, sd 0.365, both signs** | |

**Guest byte-identical in all four lanes** — `gfx_submits=11153`,
`audio_submits=7685`, `sp_tasks=18838`, `vi_interrupts=8386`,
`controller_ops=2390`, `sim_time=13112786076`, `render_error=None`, plus
identical `fields=7699` and `over_budget=3853`. The lanes differ only in host
cost, which is what makes this a valid A/B. `max` (~1.3 s in every lane) is a
startup fault-in spike that owns the statistic and is excluded from comparison.

**This is numerically the same null the flag produced before** (−0.14, sd 0.35,
both signs), now for a partly different reason and on a lane where one of the
two reconciles had been gated for the first time.

**The conclusion, which is more useful than either sign would have been:**

> `EXEC_MIRROR_NS` is 8.43 ms/render-field, but **removing its comparison
> changes the render field by nothing measurable.** Therefore **most of that
> 8.43 ms is not the comparison.**

That is rule 2 one level down: a counter can measure work that **relocates**
rather than disappears when you delete the thing the counter is named after.
The split table's apparent "largest line" is not, on this evidence, a target —
and the next person to size a candidate against it needs to know that first.

**Reverted**, on "it buys nothing" rather than "it costs": a change that moves
no measurable time while adding a divergent verification lane is not worth
carrying. The **soundness proof stands** (rules 23 and 24 keep it); only the
profit claim failed.

**The census agrees, and it is the empirical leg the source proof could not
supply.** `FN64_MIRROR_RECONCILE_CENSUS=1`, **on a 20,000-step probe**:

```
[mirror-reconcile] boundaries=19916 clean=19916 (100.0000%) dirty=0 (0.0000%)
```

**Zero dirty boundaries out of 19,916 — at 20k-step scope.** The mirror
reconcile never once caught changed state there. The falsifier had every
opportunity to fire and did not.

**Carry the scope with the number, every time.** 20,000 steps is the **boot**
portion of the route; the render-heavy region is later, and this route's own
census shows the first ~50k steps render nothing at all. So the honest claim is
*"the mirror caught nothing during boot"*, not *"the mirror never catches
anything"*. A detector idle through boot could in principle fire under
sustained rendering.

Three reasons the conclusion still holds, none of which is the census alone:
the **source proof** covers all executions and does not depend on route; the
**A/B null** was measured on the full 1.5M route *with* sustained rendering and
found no difference; and the **5.7x** barrier-to-mirror boundary ratio
(113,670 vs 19,916) bounds how much of the guard's traffic this site can
possibly own. **The scope caveat is a precision point about the evidence, not a
doubt about the conclusion.**

*(The full-route census did complete, but its output was destroyed by the
harness's output filter before it could be read — see rule 26. Quote the
full-route figure in place of this one if a later run captures it.)*

Alongside it, same run: `[mprotect-barrier] boundaries=113670 served=113669
fell_back=1 clean=90266 (79.41%) mean_dirty_pages_per_served=0.2087`. Note
**113,670 barrier boundaries against 19,916 mirror boundaries — 5.7x.** The
mirror is a *minority* of barrier traffic, which independently predicts that
gating it could not have moved much, and it did not.

**All three lines of evidence now agree:** safe to gate (source), catches
nothing (census), changes nothing measurable (A/B). A rare case where the
question is closed from three directions at once — and the answer is that this
was never the lever.

### THE MIRROR FIX: the gated comparison is a pure DETECTOR, proven from source

The redundancy proof the fix requires, done by reading before any benchmark —
because the soundness question is decidable from source and does not need a
run, while the *size* of the win does.

**What the fix is.** `reconcile_before_dispatch_from_view`
(`live_program.rs:2204`, the scheduler mirror, 8.43 ms/render-field) now gates
its comparison on `continuous_snapshot_enabled()` exactly as its twin
`reconcile_before_dispatch` (`:2160`) already did. Sealing still always runs;
`arm_barrier_over_clean_region` still always runs.

**Why gating is sound, and this is the load-bearing part.**
`reconcile_snapshot_before_dispatch` (`live_program.rs:790-882`) takes
`&mut self` but **mutates nothing**. Verified mechanically over its whole body:
zero assignments to `self`, zero collection mutations, no `commit_snapshot`, no
`adopt_snapshot`. Its entire content is three O(1) asserts and a
`recompiled_gap_panic` on the first changed range. The snapshot handed to it is
**dropped**.

So it is a **pure detector**: it either panics or does nothing observable.
Gating it removes a *check*, never a state transition — which is exactly the
property that makes "the other site does it too" more than a structural
argument. A baseline-advancing read would be a different matter entirely, and
the doc already distinguishes that case (`live_program.rs:2545-2560`); this is
not it.

#### A corruption investigation and a performance fix reached the same line from opposite directions

**This adjacency is worth more than either finding alone, and it is the reason
to trust the gate placement.** Two people, two years of context apart, working
on opposite problems — one holding a live memory corruption, one hunting 8.43
ms — drew the *same* boundary through this code and put the gate on the same
side of it.

The corruption investigation arrived at it by being burned: gating too much
produced `unjournaled executable mutation changed physical RDRAM
[0x0009b0b3, 0x0009b0b4)` at 3M steps. The performance work arrived at it by
reading for what is safe to skip. **When a safety boundary is discovered
independently by a failure and by an audit, and both land in the same place, it
is a real boundary rather than either party's convenience.**

The rule it yields, stated generally: **"is this check skippable?" is not a
property of what the check looks like — it is a property of whether the check
WRITES.** A comparison that only reads may be gated. A comparison that advances
a baseline as a side effect may never be, no matter how much it resembles the
first. The two are visually near-identical here and differ only in one call.

**Independent corroboration, written by whoever debugged the `0x0009b0b3`
failure.** The comment at `live_program.rs:2670-2685` draws exactly this
distinction, from the opposite direction — a lane that *broke*:

> "It looks like a pure 'did anything change undeclared' check, but it also
> ADVANCES the baseline: `adopt_snapshot` accepts the current bytes as
> `expected`. Skipping it leaves the baseline stale... **The reconcile check in
> `reconcile_before_dispatch` IS skippable, because it only compares and never
> advances anything. That one stays gated; this one does not.**"

That is the same comparing-vs-advancing line this fix relies on, reached by
someone who had a real corruption in hand. **Verified the mirror path falls on
the safe side of it:** `adopt_snapshot` has exactly one call site
(`live_program.rs:2810`), in the host-ABI flush the comment says must never be
gated. The scheduler mirror reaches only `reconcile_snapshot_before_dispatch`,
the pure detector. Different function, different obligation, and the gate went
on the one that compares.

**Why this is stronger than the census that was asked for, and when to reach
for which.** The brief asked for a census proving the mirror never catches
anything. A census can only ever report *"no dirty boundary was observed on the
routes I ran"* — a sampled claim, bounded by route coverage, and one that a
different route can overturn. The source argument reports *"this function
cannot change state on any input"* — a decidable claim about all executions,
established by reading the whole body. **Decidable beats sampled, and it beats
it categorically, not marginally.**

Generalizing, because the next person will reach for a census by default:

- **When the question is "does this code path ever mutate/observe X?"** —
  read the body. It is decidable, it covers every route, and it takes minutes.
- **When the question is "how often does this state actually occur, and at what
  cost?"** — census it. Frequency and magnitude are not derivable from source;
  the mirror's 8.43 ms and the ~364x barrier clipping are both facts no amount
  of reading would have produced.

Here the soundness question was the first kind and the sizing question is the
second, which is why this change carries both a source proof and an instrument.
The census remains worth running as an **independent empirical falsifier** — if
it ever reports `dirty > 0`, the source reading was wrong somewhere and that
matters more than any timing result.

**Two obligations that are NOT gated, and must not be:**

1. **`seal_with`** — establishes the baseline the journal and receipts bind to.
   Runs unconditionally, before the gate, and early-returns once sealed.
2. **`arm_barrier_over_clean_region`** — the comparison DISARMS the barrier to
   read the dirty set, so returning without re-arming would leave it down and
   let every later write accumulate into the next boundary's set instead of
   being cleared by a fresh window. The gated path re-arms on the way out.

**What still needs measurement, and what does not.** Soundness: settled above,
no run required. Size of the win: requires the A/B, and the pre-registered
falsifier stands — if the field does not move by something consistent with
8.43 ms, the mirror was cheap per call and expensive only in aggregate, and
steps-per-field is the real lever.

**The instrument that can falsify the redundancy claim empirically.**
`FN64_MIRROR_RECONCILE_CENSUS=1` counts, at the mirror site and before the gate
can skip it, how many boundaries read clean vs dirty. **`dirty > 0` on a real
route would mean this site catches state**, and the gating argument would be
wrong regardless of what the byte-identity gate says on one route. The census
is armed independently of `FN64_MPROTECT_BARRIER_STATS`, which counts barrier
asks across all sites rather than attributing outcomes to this one.

Prior evidence pointing the same way, recorded here so the census is a
confirmation rather than the only leg: write attribution over WM2000's full
route produced **505,140 journal entries with ZERO changed ranges lacking a
covering declaration** (`continuous_snapshot_enabled`'s own doc comment).

### FOUND IT: executor self is 21.72 ms — 61% of the render field, not graphics

The "unaccounted 23.94 ms" was never unmeasured. It was in the population split
already, in `/tmp/bimodal-run2-full.log` from `95e3f92`, and nobody did the
subtraction.

Render field decomposition, all figures per slow field from that run:

| | ms | share of 35.84 |
|---|---:|---:|
| `executor_ns` (**inclusive**) | 34.91 | 97% |
| — of which `gfx_ns` (nested) | 11.99 | 33% |
| — — `gfx_lle_rsp_ns` | 6.08 | 17% |
| — — `gfx_lle_rdp_ns` | 5.82 | 16% |
| — of which `audio_lle_ns` | 1.20 | 3% |
| **= EXECUTOR SELF** | **21.72** | **61%** |
| `vi_present_ns` | 0.89 | 2% |

**Executor self is 21.72 ms of a 35.84 ms field — larger than every graphics
line combined (11.99).** This is rule 2 at the top level: `executor_ns` is
inclusive, and reading it as a peer of `gfx_ns` rather than its parent hid the
single biggest cost in the program.

**And it scales with submits, so it is per-submit work, not a floor.**
`executor_calls` is **274.9/field on render fields against 68.5 on off-fields —
4.0x** — matching `gfx_calls` at 5.75/field. That works out to **79.0 µs per
executor call** on a render field. The barrier tracks it too
(`barrier_served` 991.5 vs 262.8, 3.77x).

So the correct statement of the problem: the render field is **not** dominated
by emulating the RSP or the RDP. It is dominated by whatever the executor does
**around** each of the ~5.75 graphics calls it makes per field — dispatch,
scheduling, the mutation journal, and the guard, none of which is the console's
own work.

**Next measurement: split `executor_ns` itself.** It has no sub-counters. 21.72
ms across 274.9 calls needs a breakdown by what the executor is actually doing
between phase boundaries, and that is where the 60fps bar is won or lost —
modern hardware is more than fast enough for 526k RSP instructions at 11.25 ns.

### Formerly: "67% of the render field is unaccounted for" (resolved above)

Modern hardware is orders of magnitude faster than an N64. It should not be
close. The measured lines say it is not the emulation of the console's *work*
that breaks the bar:

| line | ms | fits in 16.667 alone? |
|---|---:|---|
| RSP gfx interpretation | 6.08 | **yes, with 10.6 ms to spare** |
| raw RDP seam | 5.82 | **yes** |
| **named total** | **11.90** | |
| **UNACCOUNTED** | **23.94** | **67% of the render field** |

**Deleting both "biggest" lines entirely still leaves 23.94 ms = 1.44x
budget.** They were ranked as 62% of the *requirement*; they are only 33% of
the *field*. Sizing candidates against the 19.17 ms gap made them look
decisive when the field is 35.84 ms and most of it has no name.

Two corroborating measurements make this sharper:

- **RSP interpretation is uniform at 11.25/11.27 ns per instruction** across
  graphics and audio (`a54cf21`) — same run, same timer, −0.1% apart, against
  run-to-run drift of 6.6%. So the interpreter is **large, not slow**: 526,161
  instructions per render field at a rate with no defect in it. There is no
  interpreter bug to find.
- At ~39 cycles per RSP instruction on a ~3.5 GHz core, interpretation is
  already in the normal range for emulating a vector coprocessor
  instruction-by-instruction.

**So the next measurement is not another candidate — it is finding the 23.94
ms.** The phase counters cover graphics, audio, VI present and the guard;
whatever holds two-thirds of the render field is either outside every counter
or inside "executor self", which was 12.58 ms averaged over *all* fields and
has never been split by population.

Until that is named, every ranked candidate above is a minority share of a
field whose majority cost is unidentified.

### Candidates re-sized against the render field (19.17 ms)

| candidate | on the render field | % of 19.17 |
|---|---:|---:|
| gfx LLE — RSP microcode interpretation | **6.08 ms** | **31.7%** |
| gfx LLE — raw RDP seam | **5.82 ms** | **30.4%** |
| — of which the 8 MiB DPC staging copy | 1.71 ms | 8.9% |
| guard at the render seams (3.8x concentrated) | unsized | ? |
| `run_imem` double-decode (mean-shaver) | 0.65 ms | 3.4% |

**Those top two lines together are 11.90 ms — 62% of the entire requirement.**
Everything else on the list is a rounding error beside them.

Two sizing traps, one of which I fell into while building this table:

- **Do not double-scale.** `gfx_lle_rsp_ns` and `gfx_lle_rdp_ns` from the
  population split are *already per-render-field*. Multiplying them by
  `total/slow` again yields 12.16 and 11.64, which exceed the render field's own
  budget — an impossible result that catches the error. Only figures quoted as
  **per-field averages over all fields** need the 2x conversion.
- **Share and magnitude move in opposite directions.** The DPC copy *doubles*
  to 1.71 ms when correctly attributed to render fields only, yet its *share*
  falls from 15.1% to 8.9% because the real gap is 3.4x larger. Both numbers are
  right; quoting one as the other over- or under-sells the candidate.

### What this changes

**The goal is not 5.84 ms off the mean. It is 2.15x on the rendering field.**

If the render field were brought to exactly 16.667 ms, the mean lands at
**12.78 ms = 0.77x budget and `holds_60fps` becomes TRUE.** That is the only
path — **the fast half has no work to donate**, so no redistribution or
mean-shaving reaches the bar. Every queued candidate is a mean-shaver.

Three findings that direct the work:

- **The guard rides the submits, it is not per-field overhead.** Barrier work
  concentrates **3.8x** on slow fields (991 vs 263 boundaries).
- **Audio is flat across both populations** — `audio_tasks` 1.006x,
  `audio_lle_ns` 1.002x, `rsp_steps_audio` 1.003x. Audio runs at 60 Hz while
  rendering runs at 30 Hz, which is what proves the alternation is a **guest
  cadence** and not a host artifact.
- **`vi_present_ns` is the only counter that INVERTS: 0.513x** — present is
  more expensive on the cheap fields.

16 of 21 counters differ, but they are **not 16 independent findings** — all
are downstream of "this field carried 2.88 display lists" (`gfx_ns` 5708x,
`rsp_steps_gfx` 10819x, `dpc_calls` 4071x).

Instrument perturbation is negligible: **22.36 instrumented vs 22.41 control**.
Guest byte-identical in both.

## Leading explanation of the bimodality: a 30 Hz guest loop (CONFIRMED — see above)

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

## ANSWERED: the guest renders at 30 Hz. The split is perfect alternation.

Measured 2026-08-08 at `c2caafe` (the `fn64-audio` `codegen-units=1` lane),
route `a9e1b25e`, RT64, headless, 2.1M steps, quiet machine. Instrument:
`FN64_FRAME_CENSUS_POPULATIONS=1` plus `FN64_FRAME_CENSUS_SEQUENCE`.

**Look at the sequence before any statistic.** 400 consecutive steady-state
fields from field 2000, `S` = over budget, `f` = under:

```
SfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSf
SfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSf
SfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSf
SfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSf
SfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSfSf
```

**Zero defects in 400 fields.** Representative samples:

| field | ms | new gfx submits |
|---:|---:|---:|
| 2000 | 38.290 | 3 |
| 2001 | **8.758** | **0** |
| 2002 | 37.160 | 3 |
| 2003 | **8.354** | **0** |

**WM2000 runs its main loop at 30 Hz.** It builds a display list on every
second VI field; the off-field carries only audio, present, and the runtime
floor. That is a normal N64 design choice, not a bug, and the emulator must
still deliver 60 fields per second regardless.

### The contingency table — submits *partition* the populations

| | fast | slow |
|---|---:|---:|
| advance carried 0 new submits | **5,657** | 0 |
| advance carried >=1 | 4 | **5,659** |
| mean submits when nonzero | 1.00 | **2.88** |

100.0% of slow fields carried a submit; 0.1% of fast fields did. **Four fields
out of 11,321 break the pattern.** This is not an association, it is a
partition.

### The two populations

| | fast | slow |
|---|---:|---:|
| fields | 5,661 (50.0%) | 5,660 (50.0%) |
| mean | **8.89 ms** | **35.84 ms** |
| p50 / p95 | 8.80 / 9.63 | 36.99 / 38.33 |
| share of wall time | **19.9%** | **80.1%** |

Ratio 4.03x. **The program spends 80% of its time in 50% of its fields**, and
the off-field is at **0.53x the budget** — it has more than 7 ms of headroom.

### The prediction that was right, and the one that was wrong

The brief's hypothesis was "slow half carries ~2 submits, fast half ~1 ->
submit batching". **That is wrong**, and it was ruled out by arithmetic before
the run: one extra submit is worth ~7 ms, which would put both modes over
budget (~19 and ~26) and cannot produce a fast mode at 8.9. The surviving
version — **~2.9 vs ~0** — is what measured. The 1.44 submits/field average was
itself the tell: 1.44 = 2.88 per two fields.

### Counter ratios: 16 of 21 differ, but they are ONE finding

Every differing counter is downstream of "this field carried 2.88 display
lists": `gfx_ns` 5,708x, `rsp_steps_gfx` 10,819x, `dpc_calls` 4,071x,
`gfx_lle_rsp_ns` 10,962x. Do not read them as sixteen independent leads.

**The three that do NOT differ are the load-bearing ones:**

| counter | ratio |
|---|---:|
| `audio_tasks` | 1.006x |
| `audio_lle_ns` | 1.002x |
| `rsp_steps_audio` | 1.003x |

Audio runs flat at 60 Hz across both populations. **That is what proves the
alternation is a guest rendering cadence and not a host artifact** — a host
scheduling effect would modulate audio too.

One counter **inverts**: `vi_present_ns` is **0.513x**, i.e. presentation costs
*more* on the cheap field. Consistent with present being per-field work that
merely looks smaller beside 27 ms of rendering.

The barrier concentrates 3.8x on slow fields (991 vs 263 boundaries/field), so
**the guard is not a flat per-field tax — it rides the submits.** This retires
the reading of the 682-boundaries-per-field candidate as uniform overhead.

### Rule 16 — a statistic that cannot distinguish "random" from
### "periodic with the wrong period" is not a test for periodicity

Rule 6a in statistical clothing. Lag-1 autocorrelation was the requested
decision statistic, with a rule fixed before the data: strongly negative ->
alternation, strongly positive -> contiguous phase blocks, near zero ->
neither. Synthetic sequences through the same estimator:

| shape | lag1 | lag2 | lag3 |
|---|---:|---:|---:|
| period-2 `fSfS` | **-1.00** | +0.99 | -0.99 |
| period-4 `ffSS` | **+0.00** | -0.99 | -0.00 |
| period-3 `fSS` | **-0.50** | -0.50 | +0.99 |

**A period-4 sequence reads lag1 = +0.00 — dead centre of the "random" band —
while being perfectly periodic.** Period-3 reads -0.50, which looks like weak
alternation when the real signal is at lag 3. Reporting lag-1 alone could have
produced a confident wrong answer to the exact question being asked.

The measured lag table: **lag1 = -0.550**, lag2 +0.572, lag3 -0.558, lag4
+0.572, lag5 -0.558, lag6 +0.572. Odd lags negative, even positive, equal
magnitude — the textbook period-2 signature.

Note lag1 is **-0.550 and not -0.998** purely because each mode has internal
variance (the slow mode spans 36.5-38.3 ms). The alternation itself is
defect-free. **The coefficient understates a signal the printed string shows as
perfect** — which is the whole argument for printing the string first. Had only
the number been reported, -0.550 sits close enough to the -0.3 threshold to
invite hedging about a result that is in fact exact.

The instrument prints the raw pattern first, then lags 1..6, then the
contingency table. Decision rule and the period-3/period-4 blind spots are
pinned as tests.

### What this means for the bar

**The ceiling on any fix aimed only at the slow half is the fast mean, 8.89
ms/field = 0.53x budget — it clears the bar with 7.8 ms to spare.** So the
population split is not merely diagnostic; the fix has room.

But state the requirement correctly. The slow field must absorb **2.88 display
lists in 16.667 ms** and currently takes 35.84. That is a **2.15x reduction on
the rendering field specifically**, not a mean shave — and note the mean-based
framing (5.84 ms, 26%) understates it, because averaging the requirement across
an off-field that already has 7.8 ms of headroom flatters it.

**Every queued mean-shaver is now sized against the wrong denominator.** The
`run_imem` double-decode, the clean-boundary count and the DPC staging copy all
distribute their savings across both populations; only the ~50% landing on the
rendering field counts toward the bar.

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
- **An async guard worker.** Sound but worthless: the flag it would race
  against (`FN64_FAST_MUTATION_JOURNAL=1`) measures **−0.14 ms/render-field,
  inside noise** (three interleaved pairs, barrier ON — see "MEASURED:
  `FN64_FAST_MUTATION_JOURNAL` costs nothing with the barrier on"), and a
  thread cannot beat deletion of something that is already free. Note the flag
  does **not** delete "the whole journal": write attribution, sealing, the
  per-generation digest and two other comparison sites all keep running.
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

~~**Removing the entire guard lands near 1.27x — 47 fps, still not 60.**~~ So
"a release build without the correctness apparatus runs at hardware speed" is
false, and that inference should not be drawn from the 2.86% figure. Half the
remaining cost is *being an N64*: emulating its peripherals, its memory, and its
scheduler.

> **CORRECTED 2026-08-08 — "near 1.27x" is far too optimistic for the render
> route, and struck rather than deleted because the reasoning that produced it
> will otherwise be repeated.** Measured on the 1.5M-step rendering route with
> the executor split armed: the **entire named apparatus is 16.4%** of a
> 56.23 ms slow field (mirror 9.13 + both guard seams 0.085). Removing all of
> it leaves **46.8 ms/field = 2.8x the budget**, not 1.27x.
>
> The old figure came from self-time shares on the 19,523-step route **that
> renders nothing** (rule 11) — the same route that produced the retracted
> "1.01 ms guest code" claim. A guard share measured where there are no frames
> does not transfer to a field that has them.

Reaching 1.0x therefore needs **both** halves — and the second is now known to
be the overwhelming majority:
1. ~~the guard made cheap enough to leave on, or cleanly optional~~ — **worth
   at most 16.4% of the render field; cannot reach the bar even at zero cost**
2. genuine work on the structural half, which **nobody has attacked yet** —
   every optimization this session targeted the guard. It is **83.2%** of the
   render field (`resume NET`) and it has **no sub-counters**.

Note `FN64_FAST_MUTATION_JOURNAL=1` already measures **zero** difference
(435 ms vs 441 ms): the barrier absorbed that cost, so the removable 47% is not
sitting idle waiting to be switched off.

**CONFIRMED 2026-08-08, and this entry's stated reason was the correct one.**
Re-measured on the *rendering* route (`a9e1b25e`, RT64, barrier ON in both
lanes, three interleaved pairs): **−0.14 ms/render-field, sd 0.35, deltas
spanning both signs.** "The barrier absorbed that cost" is now traced to a
specific mechanism — the ungated scheduler-mirror reconcile arms the barrier
one step ahead of the gated call, so the gated comparison finds an empty dirty
set. See the corrected section above. Two cautions on the figures *here*
though: 435/441 ms are **whole-run totals, not ms/field**, and they come from
the 19,523-step route that renders nothing (rule 11), so this entry's number
never transferred to a rendering route in the first place — the agreement with
the new measurement is a real result, not a restatement.
