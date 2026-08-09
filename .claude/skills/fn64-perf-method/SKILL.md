---
name: fn64-perf-method
description: Use when measuring, profiling, benchmarking, or optimizing anything in fn64 — before dispatching a perf agent, before believing a benchmark number, before committing in a shared tree, or when a measurement looks surprising. Every rule earned by a specific wrong answer that was believed for hours.
---

# fn64 measurement method

Every rule below was earned by a measured failure on this project. They are not
style preferences: each one names a number that was wrong, believed, and acted
on. Full case detail — the runs, the retractions, the code sites — is in
`docs/plans/perf-method.md` (2,838 lines). Read the relevant section there
before acting on any rule you are about to lean on hard.

**The standing bar depends on the title's own render rate, and getting this
wrong reframed the whole project.** 16.667 ms per emulated VI field is
*hardware parity* — every VI field on time. But **WM2000 renders at 30 Hz**, so
a drawn frame gets **two** field budgets: **33.333 ms**. Measured render field
p50 is 36.46 ms, which is **2.19x the 60fps bar but only 1.09x the rate the
game actually draws at**.

**A DRAWN FRAME COSTS BOTH FIELDS**, and the only numbers that describe what
ships are **unprofiled means**. Measured on the shipping binary, same route,
7,699 steady fields:

| | before the mirror fix | after (`8109435`) |
|---|---:|---:|
| per-field p50 | 31.13 ms | **23.98 ms** |
| per-field p95 | 64.65 ms | 54.56 ms |
| **per-field mean** | **34.89 ms** | **27.96 ms** |
| **drawn frame (x2)** | 69.8 ms — **14.3 fps** | **55.9 ms — 17.9 fps** |
| 30fps budget | 33.33 ms | **gap: 22.6 ms** |

**Two figures quoted for hours were wrong, both optimistic, both from the same
mistake.** "45.32 ms pair / 22 fps / 11.99 ms gap" was built from **profiled
per-population p50s** (8.858 + 36.462). Profiling inflates the absolute cost,
and on a bimodal distribution **p50 is not the mean** — the mean carries the
tail that actually consumes wall time. **Never build a shipped-frame figure
from profiled numbers or from p50s.** Take an unprofiled run's mean, double it
for a 30 Hz title, and quote that.

## Running the work (not the experiment)

Earned 2026-08-08, a full day on one perf question. Every rule below is about
sequencing and coordination, not measurement — and they cost more time that day
than any measurement error did.

**Instrument, then measure, then optimize. Never interleave.** Four separate
measurements each discovered their instrument was broken *after* arming it, at
hour three, hour six, hour nine and hour twelve. Building the profiler first
would have cost one hour and saved most of them. **If the current cost of a
thing is not already in a trustworthy report, the next task is the report — not
the fix.**

**Do not dispatch an optimization for a cost you have not measured.** Three
targets were named and briefed that day; all three were wrong when measured —
the apparatus (5.1% of the field), then "translated guest code" (20.9%, briefed
as the bulk of 83%), then RDP (79% of it an artifact of a renderer the owner
does not use). Rule 1 exists; violating it in a brief is the same error as
violating it in an experiment.

**Search for existing evidence before spending machine time.** Three 25-minute
runs were dispatched to answer a question that two already-finished logs
contained. Check the log directory, the frozen snapshots and the doc before
queueing a run.

**Verify a subagent's finding before relaying it upward, or label it
unverified.** Three agent findings reached the owner as fact that day and all
three were wrong: a VI-attribution claim, a mirror regression that was a single
noisy rep, and a byte-identity deviation that was a parser bug. A relayed claim
inherits your credibility, not the reporter's.

**Fewer agents on entangled work.** Three agents sharing one worktree cost ~2
hours of holding plus a 14-minute build destroyed by a file collision the
coordinator authorized. Two agents on genuinely independent work finish sooner
than four on coupled work, because coordination is the cost.

**What worked, and should be default:** pre-registering thresholds, predictions
and falsifiers *before* data (caught three real defects; three agents retracted
their own findings unprompted); enforcing closure tolerances **in code** rather
than in intent; and a **one-minute smoke run** before any 25-minute one — one
smoke caught two defects that would each have cost a full run.

**Read your instrument's first real output before trusting it.** Three defects
that day surfaced by printing the report and looking at it, not by code review
— including a decomposition that read 45.7 ms inside a 10 ms field while every
parent/child relation held.

## The three that cost the most time

**1. Measure before dispatching, not after.** Every win on this project came
from a number; every wasted day came from reasoning about code structure. If
you cannot state the current cost of the thing you are about to optimize, you
are not ready to optimize it.

**31. Pre-register the closure tolerance IN CODE — it caught three defects in
one hour.** Buckets must sum to their parent, the residual prints
unconditionally, and the report refuses to present a split that does not close.
On three consecutive smoke runs this caught: a walking clock lapping **across a
coroutine suspend** (−697%; `resolve` read 197 ms/field inside a 40 ms field);
the **partial fix** that covered one suspend site of many (−533%); and a
**mislabelled bucket** where `gfx_ns` (21.5) exceeded its own parent (7.7).
**None was visible in the phase values — all three looked like findings.**
Corollaries: *a wall clock cannot time a stack that yields* (correct at the one
chokepoint every yield passes, never at the N call sites — N was 16 and no
enumeration is complete); *always report a nested counter against its containing
bucket*, because **a child exceeding its parent** was the only signal that fired
on the third; and **a label that is 95% something else is a wrong measurement
wearing a plausible name.**

**A frozen log is not a dead process; a rising CPU TIME is the only liveness
check.** A run reported as "a quarter through, ~75 minutes left" had in fact
been dead for 29 minutes — read from a log mtime. This route writes only on
controller edges and heartbeats, so a quiet log is normal. **`ps` with an
exact match plus an advancing `TIME` column is the check**, and it costs a
second. Related: `pgrep -f "release/wm2000-block-boot"` returns 2 with **zero**
benchmarks running because it matches your own monitoring shells — use
`pgrep -x`.

**And when a run dies, "cause unknown" beats a mechanism that fits.** The agent
whose run was killed inferred a harness reap: plausible, self-consistent, and
wrong — **the coordinator had restarted a run he did not own**, three drivers
collided, and killing the duplicates took the original down with them. There
was no direct evidence of a reap, only a dead run and an exited waiter, and a
mechanism was built to fit. *A mechanism that explains the evidence is not
thereby the cause.* Ask before diagnosing someone else's process death.

**The coordination rule this violated is primary and worth stating alone:
one owner per run.** Three agents were told to route machine access through a
single coordinator so concurrent restarts could not happen; the coordinator
then restarted an agent's run without checking whether it was already
restarting. **Cost: ~40 minutes and two dead A/Bs.** If you did not start it,
do not restart it — ask the owner.

**6c. Match the probe's LAYER to the property's layer.** Three attempts to
verify one claim, all returning clean zeros, all meaningless — and the speed and
agreement made them convincing:

| probe | result | why it could never have answered |
|---|---|---|
| `strings \| grep commit_with_optional_view` | 0 in both lanes | the function **inlines at O3**; the name is in neither file |
| `nm -U \| grep -c set_expected` | 1 in both | `nm` lists **symbols, not call sites** — one symbol called twice counts once |
| `strings \| grep <catalog digest>` | absent in both | the digest is **computed at runtime**, never a literal |

Each asked *the file* what only *the running program* or *the disassembly* can
answer. **When a probe returns zero, first ask whether it could have returned
nonzero** — that is rule 6a pointed at verification rather than at gates. Three
fast agreeing answers are more dangerous than one slow ambiguous one, and this
happened while checking a subagent's work, not while doing the work.

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

**30. A WAIT is a workload.** Rule 26 one step down: a probe is bounded, a
**polling loop is unbounded and compounds**. Symptom: *load stays high with zero
builders and zero benchmarks* — `ps -Ao comm | grep -c sleep` returned **19**,
stacked `until…sleep…done` waiters (some nested on other waiters), holding load
above the route script's own `--max-load 3.0` guard. Self-obscuring: the load
was being read to decide whether to start, and the reading moved the number — a
control loop with the wrong sign. **One waiter at a time, chained not nested**
(put the wait and the work in ONE background command so the waiter becomes the
run); **never poll a metric your polling affects**; prefer waiting on an
artifact over a machine state. Do not pass `--max-load` to escape your own
contention — that is editing the gate to match the run.

**5. Interleave A/B pairs.** Not six of A then six of B — other agents land
commits between blocks. Interleaving preserves an effect's *direction* through
contention but not its *magnitude*.

**Two or more reps, always.** A single pair reversed sign on rep 2: a −0.49 ms
"win" became −0.14 ± 0.35, inside noise.

**6b. Echo the gate's VALUE at the moment it takes effect.** One line —
`FN64_X=${FN64_X:-<unset>}` printed by the lane runner itself — caught an A/B
90 seconds in where **both lanes were the control lane.** A zsh `set -- $spec`
did not word-split, so the lane name arrived as `"armed 1"`, matched neither
branch, and fell through to the else that *unsets* the gate. Four runs, a
clean-looking A/B, a perturbation of ~0.000, and a believable fabricated
result. Rule 6 says prove the lanes differ; this is how, and it costs nothing.
Make an unrecognized lane or rep **exit non-zero** rather than defaulting into
a branch.

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

**29. FREEZE a log before analyzing it.** A file a process is still writing is
not a population. Two agents analyzing one live log got n = 117…183 — a growth
curve — and the differing counts were read as a wrong-key extraction bug
(plausible: that log carries `frame_interval_ms[` and `pump_ms[` one space
apart), producing a confident correction of a *correct* extraction. `cp` to
`$CLAUDE_JOB_DIR/tmp` and **cite the snapshot path in every claim**. Two
corollaries earned in the same hour: a gap called "clean" must be compared
against the other gaps in its own series (the "clean" one was 6.5 ms, largest
of seven >2 ms, and the distribution was trimodal not bimodal); and a series of
**windowed p50s cannot distinguish "the game got slow" from "the emulator has
regimes"** — only raw per-field data can (`FN64_FRAME_CENSUS_SEQUENCE`).

**Report per population, not just the mean.** WM2000's field distribution is
bimodal (`SfSfSfSf`, 400 fields, zero defects; 100% of slow fields carry a
submit, 0.1% of fast). An average over both populations hid the render cost for
a day. Give mean and p50/p95/p99, split by render vs off-field.

**A targeted counter improving while the program regresses does not ship.** A
rasterizer hoist moved its counter −10.21% with disjoint ranges while making the
program 3.25% slower.

## Reading a number

**32. A share of the wrong denominator is not a size — SUM the list, do not eye
it.** Having decomposed a 45.7 ms parent, I wrote that the non-graphics rows
"sum to well under the budget" and concluded the answer survived the renderer
question. Every individual number was right; **the sum was never computed.**
9.53 + 8.85 + 2.31 + … = **21.55 ms = 1.29x a 16.667 ms budget** — so an
infinitely fast renderer still misses 60fps, the opposite conclusion. Rows that
look modest against their decomposition's parent can exceed the parent the
*decision* uses. **Check every row against the denominator the decision uses
(here the field budget, not `resume NET`), and add the numbers up in a tool.**
Corollary: when you catch this, the "X is the bottleneck" framing usually
becomes "both halves must fall", which is a materially different plan.

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

> **And it under-matches too, which is the worse direction.** `pgrep -f "cargo
> build --release --bin wm2000-block-boot"` returned nothing while **15 `rustc`
> processes were running** — cargo's real argv is not the command line you
> typed, and the compile work lives in `rustc` children whose argv never
> contains your string. An over-match reads as "still busy" and costs you a
> wait; an **under-match reads as "finished" and is acted on.** Mine fired a
> "BUILD PROCESS EXITED" that was false, seconds before I would have called a
> mid-flight build a failure. **Wait on the ARTIFACT (`until [[ -x $BIN ]]`) or
> on a completion marker your own script writes — never on a process pattern**
> — and confirm liveness with advancing CPU time summed across `rustc`, not
> with the absence of a `pgrep` hit. *Absence of a match is not evidence of
> absence* (rules 19, 23).

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

On the **1.5M-step render-benchmark route** (the one `--profile` uses), verify
with `scripts/check-byte-identity.py` against `scripts/byte-identity-1p5M.txt`.
**Do not write your own extraction** — one did `findall(...)[-1]` and
manufactured a phantom failure that cost 75 minutes:

```
gfx_submits=11153  audio_submits=7685  sp_tasks=18838
vi_interrupts=8386  controller_ops=2390  sim_time=13112786076
render_error=None
```

On the older/deeper route:

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

Each was closed by measurement, not opinion.

**Every entry states its DENOMINATOR.** Audited 2026-08-08 after one entry was
found closing far more territory than it measured: *"RSP micro-optimization"*
read as *graphics is closed* while covering 17.6% of graphics and never touching
the 82% that is RDP. **An entry whose scope is narrower than its title silently
closes ground it never examined, and the whole function of this list is that
nobody re-derives its entries.** The other six were checked and are correctly
scoped — each names the specific mechanism it ruled out, not a category.

- **async RSP dispatch (ceiling 0 ms, not 13.4)** — the deadline is computed
  *from* the work: `lle.steps` does not exist until interpretation finishes, so
  the scheduler must block on the worker. The off-field is not host idle either
  — the harness *jumps* the virtual clock, so there is no slack to donate.
- **HLE graphics for THIS title's microcode** — 16,586/16,586 submits rejected
  with `NeedsLle`; 100% XBUS (`dram_dpc=0`) and 3.66 IMEM overlay swaps per
  task, so no display list exists in RDRAM to decode and no 4 KiB digest can
  identify the ucode. Admitting F3DZEX2 is a clean-room RE project, not config.
- **`FN64_FAST_MUTATION_JOURNAL` (this flag only, barrier ON in both lanes)** —
  −0.14 ms, sd 0.35, three interleaved pairs, deltas of both signs. The barrier
  absorbs it; it can only gate the site that was already cheap. *Not* a
  statement about the barrier, which is a separate A/B — conflating the two
  produced a retracted claim.
- **instruction budgeting** — changes the emulated program
- **reducing the dispatch COUNT (167.8/render field; ceiling 6.60 ms, and
  unreachable)** — the count is guest-determined: 48.3 dispatches and 15.8
  `BlockExit::HostCall` exits per SP graphics task, while `audio_lle_calls` is
  identical (0.925 vs 0.917) across populations. **The phase worth 81% of the
  bucket does not scale with the dispatch counter at all:** `exec_mirror_calls`
  == `executor_calls` (3.972x), not `resume_dispatch_calls` (5.313x), and its
  per-call cost *falls* 58.49 -> 32.32 us as the count rises — elasticity 0.570,
  because the reconcile's work is proportional to changed bytes, not to calls.
  The store-forced boundary this hunt was chasing was already removed
  2026-08-06 (`ExecutableWrite` 99.8% -> 0). Per-dispatch machinery is 2.332 ms
  = 9.1% of resume NET; `gfx_ns` is 11.626 ms = 45.6%.
- **RSP threading** — thread-local state
- **RSP micro-optimization (17.6% of graphics — NOT the renderer)** — uniform
  11.25 ns/instruction, no defect. The interpreter is **large, not slow**:
  526,161 instructions per render field at ~39 cycles each is normal.
  **Scope, added 2026-08-08 because this entry read far broader than it was
  measured:** RSP is **5.64 ms/field, 17.6% of graphics**; the RDP is
  **26.40 ms, 82.3%** and was never examined by that work. Do not read this
  line as "graphics is closed". *A closed line is dangerous in proportion to
  how broadly its title reads versus how narrowly it was measured — state the
  denominator in the title.*
- **depth-buffer copy elimination** — measured +0.84%, kept for correctness
  only (rule 12)

## Writing it down

Record negatives. "I measured X and it is not the problem" is a result and
prevents the next agent re-running it. When a figure you published turns out
wrong, **leave it visible with a correction** rather than silently swapping it —
the error is the instructive part.
