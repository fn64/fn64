---
name: fn64-perf-method
description: Use when measuring, profiling, benchmarking, or optimizing anything in fn64 — before dispatching a perf agent, before believing a benchmark number, before committing in a shared tree, or when a measurement looks surprising. Twenty-three rules, each earned by a specific wrong answer that was believed for hours.
---

# fn64 measurement method

Every rule below was earned by a measured failure on this project. They are not
style preferences: each one names a number that was wrong, believed, and acted
on. Full case detail — the runs, the retractions, the code sites — is in
`docs/plans/perf-method.md` (2,838 lines). Read the relevant section there
before acting on any rule you are about to lean on hard.

**The standing bar: 16.667 ms per emulated VI field, hardware parity.**

## The three that cost the most time

**1. Measure before dispatching, not after.** Every win on this project came
from a number; every wasted day came from reasoning about code structure. If
you cannot state the current cost of the thing you are about to optimize, you
are not ready to optimize it.

**6a. Before trusting a check, confirm it CAN fail.** A check that returns the
same answer regardless of the state it is checking is not a check. Prove the
verifier distinguishes the two outcomes it exists to distinguish — ideally by
injecting the defect and watching it catch it.

**15. Verify the state you meant to cause, not the call you made.** `kill -STOP`
returning 0 says a signal was sent, not that it landed (`ps -o stat` showing `T`
says that). A green test run in a dirty tree says nothing about HEAD. Five
instances in one day, none subtle in hindsight.

## Reading a profile

**2. Self time = count minus immediate children.** A sampling profiler
attributes samples to every frame on the stack. Reading inclusive totals as self
time produced three targets that were all artifacts. Use
`scripts/wm2000_self_time.py`.

Corollary, and it hid the single biggest cost in the program: **`executor_ns` is
INCLUSIVE.** Reading it as a peer of `gfx_ns` rather than its parent concealed
21.72 ms of a 35.84 ms field.

**3. Count, do not infer.** A profiler attributes *samples*, not *calls*.
Inferring "N calls at M ns each" got both numbers wrong. Twelve `FN64_*_CENSUS`
/ `_SYSCALLS` / `_STATS` gates exist in `fn64-abi` for this — add one.

**12. A large byte count is not a bottleneck.** Eliminating 5.92 GB of provably
dead depth-buffer copying moved the metric **+0.84%, the wrong direction**. A
bulk `memcpy` runs at streaming bandwidth; a gather-and-blend over the same
bytes does not, and only a profile distinguishes them.

**17. Do not budget instrumentation cost by counting clock reads.** A predicted
0.029 ms/field cost measured **+1.62 ms/field — wrong by 56x**. Arming a timer
in a hot loop costs what it does to inlining, register pressure and branch
layout; the clock read is the cheap part. Always run an **armed/control pair**
and correct absolutes by the ratio — shares survive, absolute ms do not. Any
phase measuring below the perturbation is below the instrument's resolution.

## Running an experiment

**4. Only measure on a quiet machine.** A concurrent shard rebuild made a 421 ms
baseline read 775 ms.

**5. Interleave A/B pairs.** Not six of A then six of B — other agents land
commits between blocks. Interleaving preserves an effect's *direction* through
contention but not its *magnitude*.

**Two or more reps, always.** A single pair reversed sign on rep 2: a −0.49 ms
"win" became −0.14 ± 0.35, inside noise.

**6. Prove the lanes differ before believing a number.** A fabricated 4.9x came
from an env gate where an empty value read as ON, so both lanes were the same
lane. Check a counter or symbol that must appear in one lane and not the other.

**19. A size difference proves "something changed", never "the thing you
meant".** A 4.4 MB delta was called proof the lanes differed; the generated
source still carried 1,872 `EXPECTED_WORDS` tables. Had it run, it would have
measured verify-on against verify-on. **Check the generated source, not the
binary size.**

**9. Never run a rebuild-triggering agent beside a benchmarking one.**
Serialize them. This produced rule 4's phantom.

**11. A frame-time distribution needs frames in it.** The standard route
(`FN64_BLOCK_MAX_STEPS=19523`) has `gfx_submits=0` — it renders nothing, so
every latency statistic over it describes an idle guest.

**Report per population, not just the mean.** WM2000's field distribution is
bimodal (`SfSfSfSf`, 400 fields, zero defects; 100% of slow fields carry a
submit, 0.1% of fast). An average over both populations hid the render cost for
a day. Give mean and p50/p95/p99, split by render vs off-field.

**A targeted counter improving while the program regresses does not ship.** A
rasterizer hoist moved its counter −10.21% with disjoint ranges while making the
program 3.25% slower.

## Reading a number

**10. State both ratios, or you have said nothing.** *Wall ms per emulated VI
field* (target 16.667) and *wall-versus-virtual* (target 1.000x) are different
questions. They diverge whenever the guest does not emit fields at its nominal
rate.

**21. The counter's UNIT is part of the counter, and a rate inherits its error
silently.** `AudioOutputStats::samples` counts i16 **channel** samples, not
frames. Read as frames it gave "91.5% of real time"; the true figure was
**45.7%**. Not a rounding apart — 91.5% sounds like jitter, 45.7% is the
emulator running at half speed. Hours were spent on the wrong diagnosis.

**22. A healthy delivery path is not a healthy stream.** `backend_buffers ==
ai_buffers` held exactly — zero drops — while the device played gaps. **Equal
counts prove nothing is being LOST; they say nothing about whether enough is
ARRIVING.** For any producer/consumer seam the health metric is *delivered ÷
wall-clock seconds against the rate the consumer demands*. Queue depths and drop
counts are downstream of it and all read clean under starvation. Put the
counter that can contradict you on the routine line, not the exceptional one.

**14. A rate-limited loop cannot report its own cost.** `shell.rs` sleeps to a
frame deadline, so `frame_interval_ms` measures the clamp, not the work — it
reads ~16.7 ms on an infinitely fast machine and a barely-adequate one alike.

**7. A line printed on a state CHANGE cannot prove absence of progress.** A
route was "deterministically stalling at controller read 600" across four runs
and three binaries. It never stalled — the harness logs only when scripted input
changes. Print on a cadence the *process* controls (`FN64_HEARTBEAT=<steps>`).

## Checking the machine and your tools

**18. `pcpu` and `pgrep -f` both lie about this workload.** `pcpu` reads 0.0
while the benchmark runs at 100% — **advancing CPU TIME is the only honest
liveness check**, sampled twice. `pgrep -f` counts your own monitoring shells
and any wrapper whose command line contains the string. A PID is not a durable
handle; PIDs get recycled.

**20. Test the verifier on this machine, not just its logic.** A gate enforcing
rule 19 crashed instead of verifying: `find -newermt "@<epoch>"` is GNU syntax
and BSD `find` on macOS rejects it. It had been tested against synthetic inputs
and passed. Prefer constructions whose failure mode is a crash over an empty
result. There is no `readelf` and no `timeout` here; binaries are Mach-O.

**A key that appears twice is two metrics.** A byte-identity checker did
`findall(...)[-1]` on `gfx_submits=` and silently compared the **steady-state
span** count (`[frame-census]`, line 175) against a **whole-run** expectation
(`[wm2000-block-progress]`, line 63). The 303 difference was exactly the
`warmup_gfx=300` fields the census excludes. It reported a guest byte-identity
failure that did not exist, and drove three 25-minute attribution runs. **Anchor
every extraction to a named line, and make a missing anchor fatal rather than
falling back to a scan.**

Its author had proven the checker could detect a wrong *value* — rule 6a done
halfway. Proving a check can fail is not proving it reads the right thing.
**And four runs agreeing was not evidence: consistency across runs is not
validity when every run goes through the same broken instrument.**

**23. Several narrow checks agreeing is not corroboration when they share a
blind spot.** Three individually-true probes concluded a commit was not on the
branch: `git log --oneline -3` (it was 15 back), a grep of `host.rs` for an
env-var string (the file imports the symbols, never names the gate), and a line
number taken from a diff hunk header rather than the current file. Each answered
the question literally asked; none answered the question meant. Ancestry is
`git merge-base --is-ancestor`, not a log window. Same family: one `runner.rs`
grepped and 0 found, when 136 of 142 carried the detector.

## Working in this tree

**8. Editing `fn64-recomp-rs` costs 32 crate rebuilds** — ~9-11 minutes, versus
~25 s for `fn64-abi`. Every file in `crates/fn64-recomp-rs/src` is a certified
source, so an edit changes an identity digest. Prefer `fn64-abi`; when you must
cross, say so in the commit.

**13. Commit with a pathspec — `git add` is not enough.** `git commit` writes
**the whole index**, not the paths you just added. A commit recording rule 12
ran `git add` on one path and landed two, because a peer had `vi.rs` staged;
**HEAD did not compile and it was pushed.** Always `git commit -- <paths>`.
Never `git add -A`.

**Background every build** (`nohup` + log) — a build was lost to a 10-minute
tool timeout. Use `$CLAUDE_JOB_DIR/tmp`, not `/tmp`, for scratch: parallel jobs
clobber `/tmp`. Give each agent its own `CARGO_TARGET_DIR`: a shared target dir
is the same contention class as a shared checkout, one layer down.

**A source edit is a build.** A shared checkout means *file* contention, not
just CPU contention — the sibling to "a diagnostic probe is a benchmark." An
agent wrote six files over ~90 seconds while a peer's 32-crate build was
reading the tree; the build died 14 minutes in on `telemetry.rs` consuming a
`PhaseTiming` field `lifecycle.rs` had not yet declared. **Editing in
dependency order would not have helped: no ordering makes a multi-file change
atomic to a concurrent reader.** The only remedy is not writing while someone
reads. Serialize writers against builders, not merely benchmarkers.

Corollary for the reader: a build that fails on a file you never opened is
someone else's in-flight edit, not your bug. Check `git status` mtimes before
debugging it — and when a peer's uncommitted work is linked into *your*
binary, name it as the first suspect if a result looks wrong.

## The environment

`.claude/local.env` is **canonical** — source it, never reconstruct. It defines
`FN64_DISCOVER_NWXE_ROM`, **not** a bare `ROM`; the shard build wants `ROM` and
panics with `ROM must name the user's NWXE image: NotPresent` without it. Builds
also need `FN64_EXECUTABLE_IMAGES` (three capture paths) and `FN64_BOOT_CONTEXT`
— see `reference/wm2000-routes/render-benchmark.zsh`.

RT64 lives at `~/Code/no-mercy-recompiled/third_party/rt64` (MIT; `build.rs`
asserts the LICENSE). The `lib/rt64` copies in jessetbh repos are UNLICENSED —
do not use them. Build from inside `examples/wm2000-block-boot`.

Do not edit `examples/wm2000-block-boot/src/main.rs` — it is hashed into
`DISPATCH_SOURCE_SHA256`.

## Guest byte-identity gate

Any change on the emulation path must reproduce these exactly, or it changed the
emulated program and does not ship:

```
gfx_submits=16586  audio_submits=11005  sp_tasks=27591
vi_interrupts=12008  controller_ops=3115  sim_time=18776001537
render_error=None
```

## Licensing — non-negotiable

fn64 stays MIT. `~/Code/aki-recomp` is **GPL-3.0**: do not open its `.c`, `.h`,
or `.cpp` files. Measured observations may be read from any project; **code may
not**. No code may be copied or adapted from any GPL runtime.

## Closed lines — do not re-propose without new evidence

Each was closed by measurement, not opinion:

- **async RSP dispatch** — the deadline is computed *from* the work
- **HLE graphics** — 16,586/16,586 submits rejected it
- **`FN64_FAST_MUTATION_JOURNAL`** — the barrier absorbs it; it can only gate
  the site that was already cheap
- **instruction budgeting** — changes the emulated program
- **RSP threading** — thread-local state
- **RSP micro-optimization** — uniform 11.25 ns/instruction, no defect. The
  interpreter is **large, not slow**: 526,161 instructions per render field at
  ~39 cycles each is normal for instruction-by-instruction vector coprocessor
  emulation.
- **depth-buffer copy elimination** — measured +0.84%, kept for correctness
  only (rule 12)

## Writing it down

Record negatives. "I measured X and it is not the problem" is a result and
prevents the next agent re-running it. When a figure you published turns out
wrong, **leave it visible with a correction** rather than silently swapping it —
the error is the instructive part.
